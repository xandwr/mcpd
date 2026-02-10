//! Tool proxy - manages subprocess communication with MCP tool servers.

use crate::mcp::{
    self, CallToolParams, CallToolResult, GetPromptParams, GetPromptResult, InitializeParams,
    InitializeResult, ListPromptsResult, ListResourcesResult, ListToolsResult, Notification,
    PROTOCOL_VERSION, Prompt, ReadResourceParams, ReadResourceResult, Request, RequestId, Resource,
    Response, Tool as McpTool,
};
use crate::registry::Tool;
use anyhow::{Context, Result, anyhow};
use serde_json::Value;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, oneshot};
use tracing::{debug, info, warn};

/// Proxy for communicating with a single MCP tool subprocess
pub struct ToolProxy {
    tool: Tool,
    state: Mutex<ProxyState>,
    /// Serializes initialization attempts so only one caller performs the handshake.
    /// Separate from `state` because `initialize()` needs to acquire `state` internally.
    init_lock: Mutex<()>,
    next_id: AtomicI64,
}

struct ProxyState {
    process: Option<Child>,
    stdin: Option<ChildStdin>,
    pending: Arc<Mutex<HashMap<i64, oneshot::Sender<Response>>>>,
    initialized: bool,
    reader_task: Option<tokio::task::JoinHandle<()>>,
}

impl ToolProxy {
    pub fn new(tool: Tool) -> Self {
        Self {
            tool,
            state: Mutex::new(ProxyState {
                process: None,
                stdin: None,
                pending: Arc::new(Mutex::new(HashMap::new())),
                initialized: false,
                reader_task: None,
            }),
            init_lock: Mutex::new(()),
            next_id: AtomicI64::new(1),
        }
    }

    /// Start the subprocess if not already running
    pub async fn start(&self) -> Result<()> {
        let mut state = self.state.lock().await;

        // Check if already running
        if let Some(ref mut child) = state.process
            && child.try_wait()?.is_none()
        {
            return Ok(());
        }

        // Abort old reader task if any
        if let Some(handle) = state.reader_task.take() {
            handle.abort();
        }

        info!(tool = %self.tool.name, command = ?self.tool.command, "Starting tool subprocess");

        let mut cmd = Command::new(&self.tool.command[0]);
        if self.tool.command.len() > 1 {
            cmd.args(&self.tool.command[1..]);
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .envs(&self.tool.env);

        let mut child = cmd
            .spawn()
            .with_context(|| format!("Failed to spawn tool: {}", self.tool.name))?;

        info!(tool = %self.tool.name, pid = ?child.id(), "Tool subprocess started");

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("Failed to capture stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("Failed to capture stdout"))?;

        state.process = Some(child);
        state.stdin = Some(stdin);
        state.initialized = false;

        // Clear old pending requests
        {
            let mut pending = state.pending.lock().await;
            for (_, tx) in pending.drain() {
                let _ = tx.send(Response::error(RequestId::Number(0), -1, "Proxy restarted"));
            }
        }

        // Spawn background reader task that owns stdout and dispatches responses
        let pending = Arc::clone(&state.pending);
        let tool_name = self.tool.name.clone();
        state.reader_task = Some(tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line).await {
                    Ok(0) => {
                        debug!(tool = %tool_name, "EOF from subprocess reader");
                        // Cancel all pending requests on EOF
                        let mut pending = pending.lock().await;
                        for (_, tx) in pending.drain() {
                            let _ = tx.send(Response::error(
                                RequestId::Number(0),
                                -1,
                                "EOF from subprocess",
                            ));
                        }
                        break;
                    }
                    Ok(_) => {
                        debug!(tool = %tool_name, line = %line.trim(), "Received line");

                        let response: Response = match serde_json::from_str(&line) {
                            Ok(r) => r,
                            Err(e) => {
                                warn!(tool = %tool_name, error = %e, line = %line.trim(), "Invalid JSON from subprocess");
                                continue;
                            }
                        };

                        let response_id = match &response.id {
                            RequestId::Number(n) => *n,
                            RequestId::String(_) => continue,
                        };

                        let mut pending = pending.lock().await;
                        if let Some(tx) = pending.remove(&response_id) {
                            let _ = tx.send(response);
                        }
                    }
                    Err(e) => {
                        warn!(tool = %tool_name, error = %e, "Read error from subprocess");
                        let mut pending = pending.lock().await;
                        for (_, tx) in pending.drain() {
                            let _ = tx.send(Response::error(
                                RequestId::Number(0),
                                -1,
                                "Read error from subprocess",
                            ));
                        }
                        break;
                    }
                }
            }
        }));

        Ok(())
    }

    /// Stop the subprocess
    pub async fn stop(&self) -> Result<()> {
        let mut state = self.state.lock().await;

        state.stdin.take();

        if let Some(handle) = state.reader_task.take() {
            handle.abort();
        }

        if let Some(mut child) = state.process.take() {
            info!(tool = %self.tool.name, "Stopping tool subprocess");
            let _ = child.kill().await;
        }

        // Cancel all pending requests
        {
            let mut pending = state.pending.lock().await;
            for (_, tx) in pending.drain() {
                let _ = tx.send(Response::error(RequestId::Number(0), -1, "Proxy stopped"));
            }
        }

        state.initialized = false;
        Ok(())
    }

    /// Perform MCP initialization handshake
    async fn initialize(&self) -> Result<InitializeResult> {
        let params = InitializeParams {
            protocol_version: PROTOCOL_VERSION.to_string(),
            capabilities: Default::default(),
            client_info: mcp::ClientInfo {
                name: "mcpd".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
        };

        let result: InitializeResult = self
            .call("initialize", Some(serde_json::to_value(params)?))
            .await?;

        info!(
            tool = %self.tool.name,
            server = %result.server_info.name,
            version = %result.server_info.version,
            "Tool initialized"
        );

        // Send initialized notification
        self.notify("notifications/initialized").await?;

        Ok(result)
    }

    /// Ensure the proxy is started and initialized.
    /// Uses a dedicated init_lock to serialize initialization attempts without
    /// holding the state lock (which initialize() needs internally).
    pub async fn ensure_ready(&self) -> Result<()> {
        self.start().await?;

        // Fast path: already initialized
        {
            let state = self.state.lock().await;
            if state.initialized {
                return Ok(());
            }
        }

        // Slow path: acquire init_lock to serialize concurrent init attempts
        let _init_guard = self.init_lock.lock().await;

        // Re-check under init_lock — another caller may have finished first
        {
            let state = self.state.lock().await;
            if state.initialized {
                return Ok(());
            }
        }

        self.initialize().await?;

        let mut state = self.state.lock().await;
        state.initialized = true;

        Ok(())
    }

    /// Send a notification (no response expected)
    async fn notify(&self, method: &str) -> Result<()> {
        let mut state = self.state.lock().await;
        let stdin = state
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow!("Process not started"))?;

        let notification = Notification::new(method);
        let mut line = serde_json::to_string(&notification)?;
        line.push('\n');

        stdin.write_all(line.as_bytes()).await?;
        stdin.flush().await?;

        debug!(tool = %self.tool.name, method, "Sent notification");
        Ok(())
    }

    /// Make a JSON-RPC call and wait for response
    pub async fn call<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<T> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let request = Request::new(id, method, params);

        let rx = {
            let mut state = self.state.lock().await;
            let stdin = state
                .stdin
                .as_mut()
                .ok_or_else(|| anyhow!("Process not started"))?;

            let mut line = serde_json::to_string(&request)?;
            line.push('\n');

            stdin.write_all(line.as_bytes()).await?;
            stdin.flush().await?;

            debug!(tool = %self.tool.name, id, method, "Sent request");

            // Set up response channel
            let (tx, rx) = oneshot::channel();
            state.pending.lock().await.insert(id, tx);

            rx
        };

        // Wait for the background reader to deliver our response
        let response = rx.await.map_err(|_| anyhow!("Response channel closed"))?;

        if let Some(err) = response.error {
            return Err(anyhow!("RPC error {}: {}", err.code, err.message));
        }

        let result = response
            .result
            .ok_or_else(|| anyhow!("No result in response"))?;

        serde_json::from_value(result).context("Failed to parse response")
    }

    /// List tools from this server
    pub async fn list_tools(&self) -> Result<Vec<McpTool>> {
        self.ensure_ready().await?;
        let result: ListToolsResult = self.call("tools/list", None).await?;
        Ok(result.tools)
    }

    /// Call a tool
    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<CallToolResult> {
        self.ensure_ready().await?;
        let params = CallToolParams {
            name: name.to_string(),
            arguments,
        };
        self.call("tools/call", Some(serde_json::to_value(params)?))
            .await
    }

    /// List resources from this server
    pub async fn list_resources(&self) -> Result<Vec<Resource>> {
        self.ensure_ready().await?;
        let result: ListResourcesResult = self.call("resources/list", None).await?;
        Ok(result.resources)
    }

    /// Read a resource
    pub async fn read_resource(&self, uri: &str) -> Result<ReadResourceResult> {
        self.ensure_ready().await?;
        let params = ReadResourceParams {
            uri: uri.to_string(),
        };
        self.call("resources/read", Some(serde_json::to_value(params)?))
            .await
    }

    /// List prompts from this server
    pub async fn list_prompts(&self) -> Result<Vec<Prompt>> {
        self.ensure_ready().await?;
        let result: ListPromptsResult = self.call("prompts/list", None).await?;
        Ok(result.prompts)
    }

    /// Get a prompt
    pub async fn get_prompt(
        &self,
        name: &str,
        arguments: std::collections::HashMap<String, String>,
    ) -> Result<GetPromptResult> {
        self.ensure_ready().await?;
        let params = GetPromptParams {
            name: name.to_string(),
            arguments,
        };
        self.call("prompts/get", Some(serde_json::to_value(params)?))
            .await
    }
}

impl Drop for ToolProxy {
    fn drop(&mut self) {
        // Abort the reader task
        if let Ok(mut state) = self.state.try_lock() {
            if let Some(handle) = state.reader_task.take() {
                handle.abort();
            }
            if let Some(mut child) = state.process.take() {
                let _ = child.start_kill();
            }
        }
    }
}
