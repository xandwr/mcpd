use crate::mcp::{
    CallToolResult, GetPromptResult, InitializeParams, InitializeResult, LEGACY_PROTOCOL_VERSION,
    Notification, PROTOCOL_VERSION, Prompt, ReadResourceResult, Request, RequestId, Resource,
    Response, RpcError, Tool as McpTool,
};
use crate::protocol::{self, CAPABILITIES, CLIENT_INFO, VERSION};
use crate::registry::Tool;
use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, mpsc, oneshot};
use tracing::{debug, warn};

type Pending = Arc<std::sync::Mutex<HashMap<i64, oneshot::Sender<Response>>>>;
type Input = mpsc::UnboundedSender<Vec<u8>>;

pub struct ToolProxy {
    tool: Tool,
    state: Mutex<ProxyState>,
    init_lock: Mutex<()>,
    next_id: AtomicI64,
    modern: Arc<AtomicBool>,
}

struct ProxyState {
    process: Option<Child>,
    stdin: Option<Input>,
    pending: Pending,
    protocol: Option<String>,
    reader_task: Option<tokio::task::JoinHandle<()>>,
    writer_task: Option<tokio::task::JoinHandle<()>>,
}

struct PendingRequest {
    id: i64,
    pending: Pending,
    stdin: Input,
}

impl Drop for PendingRequest {
    fn drop(&mut self) {
        if self.pending.lock().unwrap().remove(&self.id).is_some() {
            let message = json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": self.id}});
            let _ = write_message(&self.stdin, &message);
        }
    }
}

fn write_message(stdin: &Input, message: &impl serde::Serialize) -> Result<()> {
    let mut line = serde_json::to_vec(message)?;
    line.push(b'\n');
    stdin
        .send(line)
        .map_err(|_| anyhow!("Backend writer closed"))
}

fn fail_pending(pending: &Pending, message: &str) {
    for (id, tx) in pending.lock().unwrap().drain() {
        let _ = tx.send(Response::error(RequestId::Number(id), -32603, message));
    }
}

impl ToolProxy {
    pub fn new(tool: Tool) -> Self {
        Self {
            tool,
            state: Mutex::new(ProxyState {
                process: None,
                stdin: None,
                pending: Arc::new(std::sync::Mutex::new(HashMap::new())),
                protocol: None,
                reader_task: None,
                writer_task: None,
            }),
            init_lock: Mutex::new(()),
            next_id: AtomicI64::new(1),
            modern: Arc::new(AtomicBool::new(false)),
        }
    }

    pub async fn start(&self) -> Result<()> {
        let mut state = self.state.lock().await;
        if let Some(child) = state.process.as_mut()
            && child.try_wait()?.is_none()
        {
            return Ok(());
        }
        if let Some(handle) = state.reader_task.take() {
            handle.abort();
        }
        if let Some(handle) = state.writer_task.take() {
            handle.abort();
        }
        fail_pending(&state.pending, "Proxy restarted");
        let command = self
            .tool
            .command
            .first()
            .context("Backend command is empty")?;
        let mut child = Command::new(command)
            .args(&self.tool.command[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .envs(&self.tool.env)
            .spawn()
            .with_context(|| format!("Failed to spawn backend: {}", self.tool.name))?;
        let mut input = child.stdin.take().context("Missing stdin")?;
        let (stdin, mut outgoing) = mpsc::unbounded_channel::<Vec<u8>>();
        let pending = Arc::clone(&state.pending);
        state.writer_task = Some(tokio::spawn(async move {
            while let Some(line) = outgoing.recv().await {
                if input.write_all(&line).await.is_err() || input.flush().await.is_err() {
                    break;
                }
            }
            fail_pending(&pending, "Backend writer closed");
        }));
        let stdout = child.stdout.take().context("Missing stdout")?;
        state.stdin = Some(stdin.clone());
        state.process = Some(child);
        state.protocol = None;
        self.modern.store(false, Ordering::SeqCst);
        let modern = Arc::clone(&self.modern);
        let pending = Arc::clone(&state.pending);
        let name = self.tool.name.clone();
        state.reader_task = Some(tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(value) = serde_json::from_str::<Value>(&line) else {
                    warn!(backend = %name, "Invalid JSON from backend");
                    continue;
                };
                if value.get("method").is_some() {
                    if let Some(id) = value.get("id") {
                        if modern.load(Ordering::SeqCst) {
                            continue;
                        }
                        let reply = if value["method"] == "ping" {
                            json!({"jsonrpc": "2.0", "id": id, "result": {}})
                        } else {
                            json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "Client capability not supported"}})
                        };
                        let _ = write_message(&stdin, &reply);
                    }
                    continue;
                }
                let Ok(response) = serde_json::from_value::<Response>(value) else {
                    continue;
                };
                if let RequestId::Number(id) = response.id
                    && let Some(tx) = pending.lock().unwrap().remove(&id)
                {
                    let _ = tx.send(response);
                }
            }
            fail_pending(&pending, "Backend connection closed");
        }));
        Ok(())
    }

    pub async fn stop(&self) -> Result<()> {
        let mut state = self.state.lock().await;
        state.stdin.take();
        if let Some(handle) = state.writer_task.take() {
            handle.abort();
        }
        if let Some(handle) = state.reader_task.take() {
            handle.abort();
        }
        if let Some(mut child) = state.process.take() {
            let _ = child.kill().await;
        }
        fail_pending(&state.pending, "Proxy stopped");
        state.protocol = None;
        Ok(())
    }

    async fn negotiate(&self) -> Result<String> {
        let probe = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            self.call::<Value>(
                "server/discover",
                Some(json!({"_meta": protocol::metadata()})),
            ),
        )
        .await;
        match probe {
            Ok(Ok(result)) => {
                if result["resultType"] != "complete" {
                    return Err(anyhow!("Invalid server/discover result"));
                }
                let versions = result["supportedVersions"]
                    .as_array()
                    .context("Missing supportedVersions")?;
                if !versions.iter().any(|version| version == PROTOCOL_VERSION) {
                    return Err(anyhow!(
                        "Backend has no compatible modern protocol version: {}",
                        result["supportedVersions"]
                    ));
                }
                return Ok(PROTOCOL_VERSION.into());
            }
            Ok(Err(error))
                if error
                    .downcast_ref::<RpcError>()
                    .is_some_and(|error| matches!(error.code, -32022..=-32020)) =>
            {
                return Err(error.context(
                    "Modern backend rejected discovery; legacy fallback is not applicable",
                ));
            }
            _ => {}
        }
        let params = InitializeParams {
            protocol_version: LEGACY_PROTOCOL_VERSION.into(),
            capabilities: Default::default(),
            client_info: crate::mcp::ClientInfo {
                name: "mcpd".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
        };
        let result: InitializeResult = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            self.call("initialize", Some(serde_json::to_value(params)?)),
        )
        .await
        .context("Backend initialization timed out")??;
        if result.protocol_version != LEGACY_PROTOCOL_VERSION {
            return Err(anyhow!(
                "Unsupported legacy backend protocol version: {}",
                result.protocol_version
            ));
        }
        let stdin = self
            .state
            .lock()
            .await
            .stdin
            .clone()
            .context("Backend disconnected")?;
        write_message(&stdin, &Notification::new("notifications/initialized"))?;
        Ok(LEGACY_PROTOCOL_VERSION.into())
    }

    pub async fn ensure_ready(&self) -> Result<()> {
        let _guard = self.init_lock.lock().await;
        let unfinished = {
            let state = self.state.lock().await;
            state.process.is_some() && state.protocol.is_none()
        };
        if unfinished {
            self.stop().await?;
        }
        self.start().await?;
        if self.state.lock().await.protocol.is_none() {
            let version = self.negotiate().await?;
            debug!(backend = %self.tool.name, protocol = %version, "Backend ready");
            self.modern
                .store(version == PROTOCOL_VERSION, Ordering::SeqCst);
            self.state.lock().await.protocol = Some(version);
        }
        Ok(())
    }

    pub async fn call<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<T> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (stdin, pending, modern) = {
            let state = self.state.lock().await;
            (
                state.stdin.clone().context("Process not started")?,
                Arc::clone(&state.pending),
                state.protocol.as_deref() == Some(PROTOCOL_VERSION),
            )
        };
        let mut params = params.unwrap_or(json!({}));
        if modern {
            let object = params
                .as_object_mut()
                .context("Request parameters must be an object")?;
            let meta = object
                .entry("_meta")
                .or_insert(json!({}))
                .as_object_mut()
                .context("Request metadata must be an object")?;
            meta.insert(VERSION.into(), json!(PROTOCOL_VERSION));
            meta.entry(CAPABILITIES).or_insert(json!({}));
            meta.insert(CLIENT_INFO.into(), protocol::identity());
        }
        let request = Request::new(id, method, Some(params));
        let (tx, rx) = oneshot::channel();
        pending.lock().unwrap().insert(id, tx);
        let _guard = PendingRequest {
            id,
            pending,
            stdin: stdin.clone(),
        };
        write_message(&stdin, &request)?;
        let response = rx.await.context("Response channel closed")?;
        if let Some(error) = response.error {
            return Err(error.into());
        }
        let result = response.result.context("No result in response")?;
        if modern && !result.get("resultType").is_some_and(Value::is_string) {
            return Err(anyhow!("Modern backend omitted resultType"));
        }
        serde_json::from_value(result).context("Failed to parse response")
    }

    pub async fn request(&self, method: &str, mut params: Value) -> Result<Value> {
        self.ensure_ready().await?;
        if self.state.lock().await.protocol.as_deref() == Some(LEGACY_PROTOCOL_VERSION)
            && let Some(meta) = params.get_mut("_meta").and_then(Value::as_object_mut)
        {
            meta.remove(VERSION);
            meta.remove(CAPABILITIES);
            meta.remove(CLIENT_INFO);
        }
        self.call(method, Some(params)).await
    }

    pub async fn catalog(&self, method: &str, field: &str) -> Result<Vec<Value>> {
        let mut items = Vec::new();
        let mut params = json!({});
        let mut seen = HashSet::new();
        loop {
            let result = self.request(method, params.clone()).await?;
            items.extend(
                result[field]
                    .as_array()
                    .context("Missing catalog entries")?
                    .iter()
                    .cloned(),
            );
            match result.get("nextCursor") {
                None | Some(Value::Null) => return Ok(items),
                Some(Value::String(cursor)) if seen.insert(cursor.clone()) => {
                    params["cursor"] = json!(cursor);
                }
                _ => return Err(anyhow!("Invalid or repeated backend pagination cursor")),
            }
        }
    }

    pub async fn list_tools(&self) -> Result<Vec<McpTool>> {
        self.catalog("tools/list", "tools")
            .await?
            .into_iter()
            .map(|tool| serde_json::from_value(tool).map_err(Into::into))
            .collect()
    }

    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<CallToolResult> {
        serde_json::from_value(
            self.request("tools/call", json!({"name": name, "arguments": arguments}))
                .await?,
        )
        .map_err(Into::into)
    }

    pub async fn list_resources(&self) -> Result<Vec<Resource>> {
        self.catalog("resources/list", "resources")
            .await?
            .into_iter()
            .map(|resource| serde_json::from_value(resource).map_err(Into::into))
            .collect()
    }

    pub async fn read_resource(&self, uri: &str) -> Result<ReadResourceResult> {
        serde_json::from_value(self.request("resources/read", json!({"uri": uri})).await?)
            .map_err(Into::into)
    }

    pub async fn list_prompts(&self) -> Result<Vec<Prompt>> {
        self.catalog("prompts/list", "prompts")
            .await?
            .into_iter()
            .map(|prompt| serde_json::from_value(prompt).map_err(Into::into))
            .collect()
    }

    pub async fn get_prompt(
        &self,
        name: &str,
        arguments: HashMap<String, String>,
    ) -> Result<GetPromptResult> {
        serde_json::from_value(
            self.request("prompts/get", json!({"name": name, "arguments": arguments}))
                .await?,
        )
        .map_err(Into::into)
    }
}

impl Drop for ToolProxy {
    fn drop(&mut self) {
        if let Ok(mut state) = self.state.try_lock() {
            if let Some(handle) = state.writer_task.take() {
                handle.abort();
            }
            if let Some(handle) = state.reader_task.take() {
                handle.abort();
            }
            if let Some(mut child) = state.process.take() {
                let _ = child.start_kill();
            }
            fail_pending(&state.pending, "Proxy dropped");
        }
    }
}
