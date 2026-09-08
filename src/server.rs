use crate::discovery::{BackendError, FindToolsParams, FindToolsResult, rank_tools};
use crate::mcp::{
    LEGACY_PROTOCOL_VERSION, ListToolsResult, Notification, Request, RequestId, Response, RpcError,
    Tool as McpTool,
};
use crate::protocol::{self, CAPABILITIES, SUBSCRIPTION_ID};
use crate::proxy::BackendSession;
use crate::registry::Registry;
use anyhow::Result;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, RwLock, broadcast};
use tracing::{info, warn};

type Output = Box<dyn AsyncWrite + Unpin + Send>;

pub struct Server {
    hub: Arc<Hub>,
    shutdown_hub_on_close: bool,
    initialized: RwLock<bool>,
    subscriptions: Mutex<HashMap<RequestId, Value>>,
    output: Mutex<Output>,
}

pub struct Hub {
    registry: RwLock<Registry>,
    proxies: RwLock<HashMap<String, Arc<BackendSession>>>,
    changes: broadcast::Sender<()>,
}

fn success_or_internal_error(id: RequestId, result: &impl serde::Serialize) -> Response {
    match serde_json::to_value(result) {
        Ok(value) => Response::success(id, value),
        Err(e) => Response::error(id, -32603, format!("Serialization failed: {}", e)),
    }
}

impl Server {
    pub fn new(registry: Registry) -> Self {
        Self {
            hub: Arc::new(Hub::new(registry)),
            shutdown_hub_on_close: true,
            initialized: RwLock::new(false),
            subscriptions: Mutex::new(HashMap::new()),
            output: Mutex::new(Box::new(tokio::io::stdout())),
        }
    }

    pub fn from_hub(hub: Arc<Hub>) -> Self {
        Self::from_hub_with_output(hub, tokio::io::stdout())
    }

    pub fn from_hub_with_output<W>(hub: Arc<Hub>, output: W) -> Self
    where
        W: AsyncWrite + Unpin + Send + 'static,
    {
        Self {
            hub,
            shutdown_hub_on_close: false,
            initialized: RwLock::new(false),
            subscriptions: Mutex::new(HashMap::new()),
            output: Mutex::new(Box::new(output)),
        }
    }

    async fn write(&self, message: &impl serde::Serialize) -> Result<()> {
        let mut line = serde_json::to_vec(message)?;
        line.push(b'\n');
        let mut output = self.output.lock().await;
        output.write_all(&line).await?;
        output.flush().await?;
        Ok(())
    }

    async fn sync_registry(&self) -> Result<()> {
        self.hub.sync_registry().await?;
        Ok(())
    }

    async fn notify_registry_changed(&self) -> Result<()> {
        for (method, filter) in [
            ("notifications/tools/list_changed", "toolsListChanged"),
            (
                "notifications/resources/list_changed",
                "resourcesListChanged",
            ),
            ("notifications/prompts/list_changed", "promptsListChanged"),
        ] {
            if *self.initialized.read().await {
                self.write(&Notification::new(method)).await?;
            }
            let subscriptions = self.subscriptions.lock().await;
            for (id, filters) in subscriptions.iter() {
                if filters[filter] == true {
                    self.write(&json!({"jsonrpc": "2.0", "method": method, "params": {"_meta": {SUBSCRIPTION_ID: id}}})).await?;
                }
            }
        }
        Ok(())
    }

    fn capabilities() -> Value {
        json!({"tools": {"listChanged": true}, "resources": {"listChanged": true}, "prompts": {"listChanged": true}})
    }

    async fn subscribe(&self, request: &Request) -> Result<(), RpcError> {
        if !protocol::request_is_modern(request, *self.initialized.read().await)? {
            return Err(protocol::invalid(
                "subscriptions/listen requires modern request metadata",
            ));
        }
        let params = request.params.as_ref().unwrap();
        let filters = params
            .get("notifications")
            .and_then(Value::as_object)
            .ok_or_else(|| protocol::invalid("Missing notifications filter"))?;
        let mut accepted = json!({});
        for key in [
            "toolsListChanged",
            "resourcesListChanged",
            "promptsListChanged",
        ] {
            if let Some(value) = filters.get(key) {
                let enabled = value
                    .as_bool()
                    .ok_or_else(|| protocol::invalid("Notification filters must be boolean"))?;
                if enabled {
                    accepted[key] = json!(true);
                }
            }
        }
        if let Some(uris) = filters.get("resourceSubscriptions")
            && !uris
                .as_array()
                .is_some_and(|uris| uris.iter().all(Value::is_string))
        {
            return Err(protocol::invalid(
                "resourceSubscriptions must be an array of URI strings",
            ));
        }
        self.sync_registry()
            .await
            .map_err(|error| protocol::invalid(error.to_string()))?;
        let mut subscriptions = self.subscriptions.lock().await;
        self.write(&json!({"jsonrpc": "2.0", "method": "notifications/subscriptions/acknowledged", "params": {
            "_meta": {SUBSCRIPTION_ID: request.id}, "notifications": accepted
        }})).await.map_err(|error| protocol::invalid(error.to_string()))?;
        subscriptions.insert(request.id.clone(), accepted);
        Ok(())
    }
}

impl Hub {
    pub fn new(registry: Registry) -> Self {
        let (changes, _) = broadcast::channel(16);
        Self {
            registry: RwLock::new(registry),
            proxies: RwLock::new(HashMap::new()),
            changes,
        }
    }

    fn subscribe_changes(&self) -> broadcast::Receiver<()> {
        self.changes.subscribe()
    }

    async fn sync_registry(&self) -> Result<()> {
        let mut registry = self.registry.write().await;
        registry.reload()?;
        let names = registry.names();
        let mut proxies = self.proxies.write().await;
        let mut changed = false;
        for backend in registry.list() {
            let replaced = proxies
                .get(&backend.name)
                .is_some_and(|proxy| !proxy.matches(backend));
            if replaced && let Some(proxy) = proxies.remove(&backend.name) {
                let _ = proxy.stop().await;
            }
            if replaced || !proxies.contains_key(&backend.name) {
                proxies.insert(
                    backend.name.clone(),
                    Arc::new(BackendSession::new(backend.clone())),
                );
                changed = true;
            }
        }
        let stale: Vec<_> = proxies
            .keys()
            .filter(|name| !names.contains(*name))
            .cloned()
            .collect();
        for name in stale {
            if let Some(proxy) = proxies.remove(&name) {
                let _ = proxy.stop().await;
            }
            changed = true;
        }
        drop(proxies);
        drop(registry);
        if changed {
            let _ = self.changes.send(());
        }
        Ok(())
    }

    fn namespace_uri(server: &str, uri: &str) -> String {
        format!(
            "mcpd://{}/{}",
            server,
            uri.strip_prefix("mcpd://").unwrap_or(uri)
        )
    }

    fn namespace_content(server: &str, content: &mut Value) {
        let resource = match content["type"].as_str() {
            Some("resource_link") => content,
            Some("resource") => &mut content["resource"],
            _ => return,
        };
        if let Some(uri) = resource.get("uri").and_then(Value::as_str) {
            resource["uri"] = json!(Self::namespace_uri(server, uri));
        }
    }

    async fn aggregate_catalog(&self, method: &str, field: &str) -> Result<Vec<Value>> {
        let proxies: Vec<_> = self
            .proxies
            .read()
            .await
            .iter()
            .map(|(name, proxy)| (name.clone(), Arc::clone(proxy)))
            .collect();
        let mut all = Vec::new();
        for (server, proxy) in proxies {
            match tokio::time::timeout(
                std::time::Duration::from_secs(15),
                proxy.catalog(method, field),
            )
            .await
            {
                Ok(Ok(items)) => {
                    for mut item in items {
                        if let Some(name) = item.get("name").and_then(Value::as_str) {
                            item["name"] = json!(format!("{}__{}", server, name));
                        }
                        for key in ["uri", "uriTemplate"] {
                            if let Some(uri) = item.get(key).and_then(Value::as_str) {
                                item[key] = json!(Self::namespace_uri(&server, uri));
                            }
                        }
                        all.push(item);
                    }
                }
                Ok(Err(error)) => warn!(backend = %server, %error, "Backend catalog unavailable"),
                Err(_) => {
                    let _ = proxy.stop().await;
                    warn!(backend = %server, "Backend catalog timed out");
                }
            }
        }
        all.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        Ok(all)
    }

    fn tool_result(value: Value) -> Value {
        json!({"content": [{"type": "text", "text": value.to_string()}], "isError": false})
    }

    async fn route(&self, method: &str, mut params: Value, modern: bool) -> Result<Value> {
        let key = if method == "resources/read" {
            "uri"
        } else {
            "name"
        };
        let qualified = params[key]
            .as_str()
            .ok_or_else(|| protocol::invalid(format!("Missing {}", key)))?
            .to_string();
        let (server, original) = if method == "resources/read" {
            qualified
                .strip_prefix("mcpd://")
                .and_then(|uri| uri.split_once('/'))
                .ok_or_else(|| protocol::invalid("Expected mcpd://server/uri"))?
        } else {
            qualified
                .split_once("__")
                .ok_or_else(|| protocol::invalid("Expected server__name"))?
        };
        let proxy = self
            .proxies
            .read()
            .await
            .get(server)
            .cloned()
            .ok_or_else(|| protocol::invalid(format!("Unknown server '{}'", server)))?;
        params[key] = json!(original);
        if !modern {
            params["_meta"] = json!({CAPABILITIES: {}});
        }
        if let Some(capabilities) = params
            .get_mut("_meta")
            .and_then(|meta| meta.get_mut(CAPABILITIES))
            .and_then(Value::as_object_mut)
        {
            capabilities.remove("extensions");
        }
        let mut result = proxy.request(method, params.clone()).await?;
        if result["resultType"] == "input_required" {
            if !modern {
                return Err(anyhow::anyhow!(
                    "Backend requires a client supporting MCP 2026-07-28 multi round-trip requests"
                ));
            }
            if result.get("inputRequests").is_none() && result.get("requestState").is_none() {
                return Err(anyhow::anyhow!(
                    "Backend returned input_required without input requests or state"
                ));
            }
            if result
                .get("requestState")
                .is_some_and(|state| !state.is_string())
                || result
                    .get("inputRequests")
                    .is_some_and(|inputs| !inputs.is_object())
            {
                return Err(anyhow::anyhow!(
                    "Backend returned malformed input_required fields"
                ));
            }
            if let Some(requests) = result.get("inputRequests").and_then(Value::as_object) {
                for input in requests.values() {
                    let capability = match input["method"].as_str() {
                        Some("elicitation/create") => "elicitation",
                        Some("roots/list") => "roots",
                        Some("sampling/createMessage") => "sampling",
                        _ => {
                            return Err(anyhow::anyhow!(
                                "Backend returned an unknown input request"
                            ));
                        }
                    };
                    let declared = params["_meta"][CAPABILITIES]
                        .get(capability)
                        .and_then(Value::as_object);
                    let supported = declared.is_some_and(|declared| {
                        if capability != "elicitation" {
                            return true;
                        }
                        match input["params"]["mode"].as_str().unwrap_or("form") {
                            "form" => {
                                declared.is_empty()
                                    || declared.get("form").is_some_and(Value::is_object)
                            }
                            "url" => declared.get("url").is_some_and(Value::is_object),
                            _ => false,
                        }
                    });
                    if !supported {
                        return Err(RpcError {
                            code: -32021,
                            message: "Missing required client capability".into(),
                            data: Some(json!({"requiredCapabilities": {capability: {}}})),
                        }
                        .into());
                    }
                }
            }
            return Ok(result);
        }
        if let Some(contents) = result.get_mut("content").and_then(Value::as_array_mut) {
            for content in contents {
                Self::namespace_content(server, content);
            }
        }
        if let Some(messages) = result.get_mut("messages").and_then(Value::as_array_mut) {
            for message in messages {
                if let Some(content) = message.get_mut("content") {
                    Self::namespace_content(server, content);
                }
            }
        }
        if method == "resources/read"
            && let Some(contents) = result.get_mut("contents").and_then(Value::as_array_mut)
        {
            for content in contents {
                if let Some(uri) = content.get("uri").and_then(Value::as_str) {
                    content["uri"] = json!(Self::namespace_uri(server, uri));
                }
            }
        }
        if method == "tools/call" {
            if let Some(error) = result
                .as_object_mut()
                .and_then(|object| object.remove("is_error"))
            {
                result["isError"] = error;
            }
            result
                .as_object_mut()
                .ok_or_else(|| anyhow::anyhow!("Invalid backend result"))?
                .entry("isError")
                .or_insert(json!(false));
        }
        Ok(result)
    }
}

impl Server {
    async fn dispatch(&self, request: &Request, modern: bool) -> Result<Value> {
        let params = request.params.clone().unwrap_or(json!({}));
        match request.method.as_str() {
            "server/discover" => Ok(
                json!({"supportedVersions": protocol::supported_versions(), "capabilities": Self::capabilities(),
                "instructions": "Search registered capabilities with find_tools, then call use_tool with a returned server__tool name and its arguments."}),
            ),
            "initialize" if !modern => {
                serde_json::from_value::<crate::mcp::InitializeParams>(params)
                    .map_err(|error| protocol::invalid(error.to_string()))?;
                *self.initialized.write().await = true;
                Ok(
                    json!({"protocolVersion": LEGACY_PROTOCOL_VERSION, "capabilities": Self::capabilities(), "serverInfo": protocol::identity()}),
                )
            }
            "ping" if !modern => Ok(json!({})),
            "tools/list" => Ok(self
                .handle_list_tools(request.id.clone())
                .await
                .result
                .unwrap()),
            "tools/call" => {
                let name = params["name"]
                    .as_str()
                    .ok_or_else(|| protocol::invalid("Missing tool name"))?;
                let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
                if !arguments.is_object() {
                    return Err(protocol::invalid("Tool arguments must be an object").into());
                }
                match name {
                    "find_tools" => {
                        let search: FindToolsParams = serde_json::from_value(arguments)
                            .map_err(|error| protocol::invalid(error.to_string()))?;
                        search.validate().map_err(protocol::invalid)?;
                        let found = self.find_tools(&search).await.map_err(anyhow::Error::msg)?;
                        Ok(Hub::tool_result(serde_json::to_value(found)?))
                    }
                    "list_tools" => {
                        self.sync_registry().await?;
                        let mut tools = self.hub.aggregate_catalog("tools/list", "tools").await?;
                        for tool in &mut tools {
                            if let Some(schema) = tool
                                .as_object_mut()
                                .and_then(|tool| tool.remove("inputSchema"))
                            {
                                tool["input_schema"] = schema;
                            }
                        }
                        Ok(Hub::tool_result(json!(tools)))
                    }
                    "use_tool" => {
                        let tool = arguments["tool_name"]
                            .as_str()
                            .ok_or_else(|| protocol::invalid("Missing tool_name"))?;
                        let mut backend_params = params.clone();
                        backend_params["name"] = json!(tool);
                        backend_params["arguments"] =
                            arguments.get("arguments").cloned().unwrap_or(json!({}));
                        self.sync_registry().await?;
                        self.hub.route("tools/call", backend_params, modern).await
                    }
                    _ => Err(protocol::invalid(format!(
                        "Unknown tool '{}'. Use find_tools, list_tools, or use_tool.",
                        name
                    ))
                    .into()),
                }
            }
            "resources/list" | "resources/templates/list" | "prompts/list" => {
                let field = match request.method.as_str() {
                    "resources/list" => "resources",
                    "resources/templates/list" => "resourceTemplates",
                    _ => "prompts",
                };
                self.sync_registry().await?;
                Ok(json!({field: self.hub.aggregate_catalog(&request.method, field).await?}))
            }
            "resources/read" | "prompts/get" => {
                self.sync_registry().await?;
                self.hub.route(&request.method, params, modern).await
            }
            _ => Err(RpcError {
                code: -32601,
                message: format!("Unknown method: {}", request.method),
                data: None,
            }
            .into()),
        }
    }

    async fn handle_request(&self, request: Request) -> Response {
        let modern = match protocol::request_is_modern(&request, *self.initialized.read().await) {
            Ok(modern) => modern,
            Err(error) => return protocol::error_response(request.id, error),
        };
        let mut response = match self.dispatch(&request, modern).await {
            Ok(result) => Response::success(request.id, result),
            Err(error) => {
                if let Some(rpc) = error.downcast_ref::<RpcError>() {
                    protocol::error_response(request.id, rpc.clone())
                } else if request.method == "tools/call" {
                    Response::success(
                        request.id,
                        json!({"content": [{"type": "text", "text": error.to_string()}], "isError": true}),
                    )
                } else {
                    Response::error(request.id, -32603, error.to_string())
                }
            }
        };
        protocol::finish(&mut response, &request.method, modern);
        response
    }

    pub async fn run(self) -> Result<()> {
        self.run_with(tokio::io::stdin()).await
    }

    pub async fn run_with<R>(self, input: R) -> Result<()>
    where
        R: AsyncRead + Unpin,
    {
        let server = Arc::new(self);
        let mut lines = BufReader::new(input).lines();
        let mut changes = server.hub.subscribe_changes();
        let mut tasks = tokio::task::JoinSet::new();
        let mut active: HashMap<RequestId, tokio::task::AbortHandle> = HashMap::new();
        loop {
            tokio::select! {
                changed = changes.recv() => {
                    match changed {
                        Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) => {
                            server.notify_registry_changed().await?;
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                completed = tasks.join_next(), if !tasks.is_empty() => {
                    if let Some(Ok((id, response))) = completed
                        && active.remove(&id).is_some()
                    {
                        server.write(&response).await?;
                    }
                }
                line = lines.next_line() => {
                    let Some(line) = line? else { break };
                    if line.trim().is_empty() { continue; }
                    let value = match serde_json::from_str::<Value>(&line) {
                        Ok(value) => value,
                        Err(_) => {
                            server.write(&Response::error(RequestId::Null, -32700, "Parse error")).await?;
                            continue;
                        }
                    };
                    if value.get("id").is_none() && value.get("method").is_some() {
                        if value["jsonrpc"] != "2.0" { continue; }
                        if value["method"] == "notifications/cancelled"
                            && let Ok(id) = serde_json::from_value::<RequestId>(value["params"]["requestId"].clone())
                        {
                            if let Some(handle) = active.remove(&id) { handle.abort(); }
                            server.subscriptions.lock().await.remove(&id);
                        }
                        continue;
                    }
                    let request = match serde_json::from_value::<Request>(value) {
                        Ok(request) => request,
                        Err(_) => {
                            server.write(&Response::error(RequestId::Null, -32600, "Invalid request")).await?;
                            continue;
                        }
                    };
                    if active.contains_key(&request.id) || server.subscriptions.lock().await.contains_key(&request.id) {
                        server.write(&Response::error(request.id, -32600, "Request ID already in use")).await?;
                        continue;
                    }
                    if request.method == "subscriptions/listen" {
                        if let Err(error) = server.subscribe(&request).await {
                            server.write(&protocol::error_response(request.id, error)).await?;
                        }
                    } else if request.method == "initialize" {
                        let response = server.handle_request(request).await;
                        server.write(&response).await?;
                    } else {
                        let id = request.id.clone();
                        let server = Arc::clone(&server);
                        let handle = tasks.spawn(async move {
                            let response = server.handle_request(request).await;
                            (response.id.clone(), response)
                        });
                        active.insert(id, handle);
                    }
                }
            }
        }
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
        for (id, _) in server.subscriptions.lock().await.drain() {
            let mut response =
                Response::success(id.clone(), json!({"_meta": {SUBSCRIPTION_ID: id}}));
            protocol::finish(&mut response, "subscriptions/listen", true);
            server.write(&response).await?;
        }
        if server.shutdown_hub_on_close {
            server.hub.shutdown().await;
        }
        Ok(())
    }
    async fn handle_list_tools(&self, id: RequestId) -> Response {
        let tools = vec![
            McpTool {
                extra: Default::default(),
                name: "find_tools".to_string(),
                description: Some(
                    "Find capabilities available through locally registered MCP servers. \
                     Search before deciding you lack a tool for a task. Matches keywords against \
                     server names, tool names, and descriptions, ranked by relevance. \
                     Returns up to 10 matches with input schemas, registered server names, and \
                     backend errors. Omit query to browse. Use server to scope discovery, \
                     then invoke a matching server__tool with use_tool."
                        .to_string(),
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "Space-separated keywords, such as 'file search' or 'scene inspect'. Matches any keyword; names rank above descriptions."},
                        "server": {"type": "string", "description": "Optional exact registered server name. Only this backend will be queried."},
                        "limit": {"type": "integer", "minimum": 1, "maximum": 100, "default": 10}
                    },
                    "additionalProperties": false
                }),
            },
            McpTool {
                extra: Default::default(),
                name: "list_tools".to_string(),
                description: Some(
                    "List all available tools from registered MCP backends. \
                     Returns tool names, descriptions, and input schemas. \
                     Call this first to discover what tools are available, \
                     then use `use_tool` to invoke them."
                        .to_string(),
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }),
            },
            McpTool {
                extra: Default::default(),
                name: "use_tool".to_string(),
                description: Some(
                    "Invoke a tool by name. Use `find_tools` or `list_tools` first to discover \
                     available tools and their expected arguments."
                        .to_string(),
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "tool_name": {
                            "type": "string",
                            "description": "The fully-qualified tool name (server__tool) as returned by list_tools"
                        },
                        "arguments": {
                            "type": "object",
                            "description": "Arguments to pass to the tool, matching its input schema"
                        }
                    },
                    "required": ["tool_name"],
                    "additionalProperties": false
                }),
            },
        ];

        info!(count = tools.len(), "Serving static meta-tools");

        let result = ListToolsResult { tools };
        success_or_internal_error(id, &result)
    }

    pub async fn find_tools(&self, params: &FindToolsParams) -> Result<FindToolsResult, String> {
        self.sync_registry()
            .await
            .map_err(|error| error.to_string())?;
        self.hub.find_tools(params).await
    }
}

impl Hub {
    pub async fn shutdown(&self) {
        for proxy in self.proxies.read().await.values() {
            let _ = proxy.stop().await;
        }
    }

    async fn find_tools(&self, params: &FindToolsParams) -> Result<FindToolsResult, String> {
        params.validate()?;
        let proxies = self.proxies.read().await;
        let servers: Vec<_> = proxies.keys().cloned().collect();
        if let Some(server) = &params.server
            && !proxies.contains_key(server)
        {
            return Err(format!(
                "Unknown server '{}'. Omit server to discover registered servers.",
                server
            ));
        }
        let mut tasks = tokio::task::JoinSet::new();
        for (name, proxy) in proxies.iter() {
            if params.server.as_ref().is_some_and(|server| server != name) {
                continue;
            }
            let name = name.clone();
            let proxy = Arc::clone(proxy);
            tasks.spawn(async move {
                let result = match tokio::time::timeout(
                    std::time::Duration::from_secs(15),
                    proxy.list_tools(),
                )
                .await
                {
                    Ok(result) => result.map_err(|e| e.to_string()),
                    Err(_) => {
                        let _ = proxy.stop().await;
                        Err("Tool discovery timed out after 15 seconds".to_string())
                    }
                };
                (name, result)
            });
        }
        drop(proxies);
        let mut catalogs = Vec::new();
        let mut errors = Vec::new();
        while let Some(result) = tasks.join_next().await {
            let (server, result) = result.map_err(|e| format!("Discovery task failed: {}", e))?;
            match result {
                Ok(tools) => catalogs.push((server, tools)),
                Err(error) => errors.push(BackendError { server, error }),
            }
        }
        Ok(rank_tools(params, catalogs, servers, errors))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::RequestId;
    use tempfile::TempDir;

    #[tokio::test]
    async fn shared_servers_keep_connection_state_isolated() {
        let directory = TempDir::new().unwrap();
        let registry = Registry::load_from(directory.path().join("registry.json")).unwrap();
        let hub = Arc::new(Hub::new(registry));
        let first = Server::from_hub(Arc::clone(&hub));
        let second = Server::from_hub(Arc::clone(&hub));

        *first.initialized.write().await = true;
        first
            .subscriptions
            .lock()
            .await
            .insert(RequestId::Number(1), json!({"toolsListChanged": true}));

        assert!(Arc::ptr_eq(&first.hub, &second.hub));
        assert!(*first.initialized.read().await);
        assert!(!*second.initialized.read().await);
        assert_eq!(first.subscriptions.lock().await.len(), 1);
        assert!(second.subscriptions.lock().await.is_empty());
        assert!(!first.shutdown_hub_on_close);
        assert!(!second.shutdown_hub_on_close);
    }

    #[test]
    fn namespace_uri_normal() {
        let result = Hub::namespace_uri("myserver", "file:///test.txt");
        assert_eq!(result, "mcpd://myserver/file:///test.txt");
    }

    #[test]
    fn namespace_uri_already_prefixed() {
        let result = Hub::namespace_uri("myserver", "mcpd://other/resource");
        assert_eq!(result, "mcpd://myserver/other/resource");
    }

    #[test]
    fn namespace_uri_empty() {
        let result = Hub::namespace_uri("srv", "");
        assert_eq!(result, "mcpd://srv/");
    }

    #[test]
    fn success_or_internal_error_with_valid_value() {
        let id = RequestId::Number(1);
        let response = success_or_internal_error(id, &"hello");
        assert!(response.result.is_some());
        assert!(response.error.is_none());
    }

    #[test]
    fn success_or_internal_error_with_unserializable_value() {
        struct AlwaysFail;
        impl serde::Serialize for AlwaysFail {
            fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("intentional failure"))
            }
        }

        let id = RequestId::Number(42);
        let response = success_or_internal_error(id, &AlwaysFail);
        assert!(response.error.is_some());
        assert!(response.result.is_none());
        let err = response.error.unwrap();
        assert_eq!(err.code, -32603);
        assert!(err.message.contains("Serialization failed"));
    }
}
