//! Aggregating MCP server - exposes two meta-tools: list_tools and use_tool.

use crate::mcp::{
    CallToolParams, CallToolResult, Content, InitializeResult, ListToolsResult, Notification,
    PROTOCOL_VERSION, Request, RequestId, Response, ServerCapabilities, ServerInfo,
    Tool as McpTool, ToolsCapability,
};
use crate::proxy::ToolProxy;
use crate::registry::Registry;
use anyhow::Result;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// Aggregating MCP server that exposes two static tools:
/// - `list_tools`: discover all available tools from registered backends
/// - `use_tool`: call any discovered tool by name
pub struct Server {
    registry: Arc<RwLock<Registry>>,
    proxies: RwLock<HashMap<String, Arc<ToolProxy>>>,
    initialized: RwLock<bool>,
}

impl Server {
    pub fn new(registry: Registry) -> Self {
        Self {
            registry: Arc::new(RwLock::new(registry)),
            proxies: RwLock::new(HashMap::new()),
            initialized: RwLock::new(false),
        }
    }

    /// Ensure all registered tools have proxies
    async fn ensure_proxies(&self) -> Result<()> {
        let registry = self.registry.read().await;
        let mut proxies = self.proxies.write().await;

        for tool in registry.list() {
            if !proxies.contains_key(&tool.name) {
                info!(tool = %tool.name, "Creating proxy");
                proxies.insert(tool.name.clone(), Arc::new(ToolProxy::new(tool.clone())));
            }
        }

        Ok(())
    }

    /// Handle initialize request
    async fn handle_initialize(&self, id: RequestId) -> Response {
        *self.initialized.write().await = true;

        let result = InitializeResult {
            protocol_version: PROTOCOL_VERSION.to_string(),
            capabilities: ServerCapabilities {
                tools: Some(ToolsCapability {
                    list_changed: false,
                }),
            },
            server_info: ServerInfo {
                name: "mcpd".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
        };

        Response::success(id, serde_json::to_value(result).unwrap())
    }

    /// Handle tools/list - returns our two static meta-tools
    async fn handle_list_tools(&self, id: RequestId) -> Response {
        let tools = vec![
            McpTool {
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
                name: "use_tool".to_string(),
                description: Some(
                    "Invoke a tool by name. Use `list_tools` first to discover \
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

        info!(count = 2, "Serving static meta-tools");

        let result = ListToolsResult { tools };
        Response::success(id, serde_json::to_value(result).unwrap())
    }

    /// Aggregate tools from all backend proxies
    async fn aggregate_backend_tools(&self) -> Result<Vec<serde_json::Value>, String> {
        if let Err(e) = self.ensure_proxies().await {
            return Err(format!("Failed to ensure proxies: {}", e));
        }

        let proxies = self.proxies.read().await;
        let mut all_tools = Vec::new();

        for (proxy_name, proxy) in proxies.iter() {
            match proxy.list_tools().await {
                Ok(tools) => {
                    for tool in tools {
                        let prefixed_name = format!("{}__{}", proxy_name, tool.name);
                        all_tools.push(json!({
                            "name": prefixed_name,
                            "description": tool.description.unwrap_or_default(),
                            "input_schema": tool.input_schema,
                        }));
                    }
                }
                Err(e) => {
                    warn!(proxy = %proxy_name, error = %e, "Failed to list tools from proxy");
                }
            }
        }

        info!(count = all_tools.len(), "Aggregated tools from all backends");
        Ok(all_tools)
    }

    /// Route a use_tool call to the appropriate backend
    async fn route_tool_call(
        &self,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<CallToolResult, String> {
        // Parse "proxyname__toolname" format
        let (proxy_name, original_name) = tool_name
            .split_once("__")
            .ok_or_else(|| format!(
                "Invalid tool name '{}'. Expected format: server__tool. Use list_tools to see available tools.",
                tool_name
            ))?;

        let proxy = {
            if let Err(e) = self.ensure_proxies().await {
                return Err(format!("Failed to ensure proxies: {}", e));
            }
            let proxies = self.proxies.read().await;
            proxies
                .get(proxy_name)
                .cloned()
                .ok_or_else(|| format!("Unknown server '{}'. Use list_tools to see available tools.", proxy_name))?
        };

        proxy
            .call_tool(original_name, arguments)
            .await
            .map_err(|e| format!("Tool call failed: {}", e))
    }

    /// Handle tools/call request - dispatches list_tools and use_tool
    async fn handle_call_tool(&self, id: RequestId, params: CallToolParams) -> Response {
        match params.name.as_str() {
            "list_tools" => match self.aggregate_backend_tools().await {
                Ok(tools) => {
                    let result = CallToolResult {
                        content: vec![Content::Text {
                            text: serde_json::to_string_pretty(&tools).unwrap(),
                        }],
                        is_error: false,
                    };
                    Response::success(id, serde_json::to_value(result).unwrap())
                }
                Err(e) => {
                    let result = CallToolResult {
                        content: vec![Content::Text {
                            text: format!("Error listing tools: {}", e),
                        }],
                        is_error: true,
                    };
                    Response::success(id, serde_json::to_value(result).unwrap())
                }
            },
            "use_tool" => {
                let tool_name = match params.arguments.get("tool_name").and_then(|v| v.as_str()) {
                    Some(name) => name.to_string(),
                    None => {
                        let result = CallToolResult {
                            content: vec![Content::Text {
                                text: "Missing required parameter 'tool_name'. Use list_tools to discover available tools.".to_string(),
                            }],
                            is_error: true,
                        };
                        return Response::success(id, serde_json::to_value(result).unwrap());
                    }
                };

                let arguments = params
                    .arguments
                    .get("arguments")
                    .cloned()
                    .unwrap_or(json!({}));

                match self.route_tool_call(&tool_name, arguments).await {
                    Ok(result) => Response::success(id, serde_json::to_value(result).unwrap()),
                    Err(e) => {
                        error!(tool = %tool_name, error = %e, "use_tool failed");
                        let result = CallToolResult {
                            content: vec![Content::Text {
                                text: format!("Error: {}", e),
                            }],
                            is_error: true,
                        };
                        Response::success(id, serde_json::to_value(result).unwrap())
                    }
                }
            }
            other => {
                let result = CallToolResult {
                    content: vec![Content::Text {
                        text: format!(
                            "Unknown tool '{}'. mcpd exposes two tools: list_tools and use_tool.",
                            other
                        ),
                    }],
                    is_error: true,
                };
                Response::success(id, serde_json::to_value(result).unwrap())
            }
        }
    }

    /// Handle a single request
    async fn handle_request(&self, request: Request) -> Response {
        debug!(method = %request.method, id = ?request.id, "Handling request");

        match request.method.as_str() {
            "initialize" => self.handle_initialize(request.id).await,
            "tools/list" => self.handle_list_tools(request.id).await,
            "tools/call" => {
                let params: CallToolParams = match request.params {
                    Some(p) => match serde_json::from_value(p) {
                        Ok(params) => params,
                        Err(e) => {
                            return Response::error(
                                request.id,
                                -32602,
                                format!("Invalid params: {}", e),
                            );
                        }
                    },
                    None => {
                        return Response::error(request.id, -32602, "Missing params");
                    }
                };
                self.handle_call_tool(request.id, params).await
            }
            _ => Response::error(
                request.id,
                -32601,
                format!("Unknown method: {}", request.method),
            ),
        }
    }

    /// Handle a notification (no response)
    async fn handle_notification(&self, notification: Notification) {
        debug!(method = %notification.method, "Handling notification");

        match notification.method.as_str() {
            "notifications/initialized" => {
                info!("Client initialized");
            }
            "notifications/cancelled" => {
                // Handle cancellation if needed
            }
            _ => {
                debug!(method = %notification.method, "Unknown notification");
            }
        }
    }

    /// Run the server on stdio
    pub async fn run(&self) -> Result<()> {
        let stdin = tokio::io::stdin();
        let mut stdout = tokio::io::stdout();
        let mut reader = BufReader::new(stdin);

        info!("MCP server starting on stdio");

        loop {
            let mut line = String::new();
            let bytes_read = reader.read_line(&mut line).await?;

            if bytes_read == 0 {
                info!("EOF received, shutting down");
                break;
            }

            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            debug!(line = %line, "Received message");

            // Try to parse as request first
            if let Ok(request) = serde_json::from_str::<Request>(line) {
                let response = self.handle_request(request).await;
                let mut response_line = serde_json::to_string(&response)?;
                response_line.push('\n');
                stdout.write_all(response_line.as_bytes()).await?;
                stdout.flush().await?;
                continue;
            }

            // Try as notification
            if let Ok(notification) = serde_json::from_str::<Notification>(line) {
                self.handle_notification(notification).await;
                continue;
            }

            warn!(line = %line, "Failed to parse message");
        }

        // Clean up proxies
        let proxies = self.proxies.read().await;
        for proxy in proxies.values() {
            let _ = proxy.stop().await;
        }

        Ok(())
    }
}
