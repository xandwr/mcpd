//! Tool proxy - manages subprocess communication with MCP tool servers.

use crate::mcp::{
    self, CallToolParams, CallToolResult, InitializeParams, InitializeResult, ListToolsResult,
    Notification, PROTOCOL_VERSION, Request, RequestId, Response, Tool as McpTool,
};
use crate::registry::Tool;
use anyhow::{Context, Result, anyhow};
use serde_json::Value;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, oneshot};
use tracing::{debug, info};

/// Proxy for communicating with a single MCP tool subprocess
pub struct ToolProxy {
    tool: Tool,
    state: Mutex<ProxyState>,
    next_id: AtomicI64,
}

struct ProxyState {
    process: Option<Child>,
    pending: HashMap<i64, oneshot::Sender<Response>>,
    initialized: bool,
}

impl ToolProxy {
    pub fn new(tool: Tool) -> Self {
        Self {
            tool,
            state: Mutex::new(ProxyState {
                process: None,
                pending: HashMap::new(),
                initialized: false,
            }),
            next_id: AtomicI64::new(1),
        }
    }

    /// Start the subprocess if not already running
    pub async fn start(&self) -> Result<()> {
        let mut state = self.state.lock().await;

        // Check if already running
        if let Some(ref mut child) = state.process {
            if child.try_wait()?.is_none() {
                return Ok(());
            }
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

        let child = cmd
            .spawn()
            .with_context(|| format!("Failed to spawn tool: {}", self.tool.name))?;

        info!(tool = %self.tool.name, pid = ?child.id(), "Tool subprocess started");

        state.process = Some(child);
        state.initialized = false;
        state.pending.clear();

        Ok(())
    }

    /// Stop the subprocess
    pub async fn stop(&self) -> Result<()> {
        let mut state = self.state.lock().await;

        if let Some(mut child) = state.process.take() {
            info!(tool = %self.tool.name, "Stopping tool subprocess");
            let _ = child.kill().await;
        }

        // Cancel all pending requests
        for (_, tx) in state.pending.drain() {
            let _ = tx.send(Response::error(RequestId::Number(0), -1, "Proxy stopped"));
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

    /// Ensure the proxy is started and initialized
    pub async fn ensure_ready(&self) -> Result<()> {
        self.start().await?;

        let needs_init = {
            let state = self.state.lock().await;
            !state.initialized
        };

        if needs_init {
            self.initialize().await?;
            let mut state = self.state.lock().await;
            state.initialized = true;
        }

        Ok(())
    }

    /// Send a notification (no response expected)
    async fn notify(&self, method: &str) -> Result<()> {
        let mut state = self.state.lock().await;
        let process = state
            .process
            .as_mut()
            .ok_or_else(|| anyhow!("Process not started"))?;

        let stdin = process.stdin.as_mut().ok_or_else(|| anyhow!("No stdin"))?;

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
            let process = state
                .process
                .as_mut()
                .ok_or_else(|| anyhow!("Process not started"))?;

            let stdin = process.stdin.as_mut().ok_or_else(|| anyhow!("No stdin"))?;

            let mut line = serde_json::to_string(&request)?;
            line.push('\n');

            stdin.write_all(line.as_bytes()).await?;
            stdin.flush().await?;

            debug!(tool = %self.tool.name, id, method, "Sent request");

            // Set up response channel
            let (tx, rx) = oneshot::channel();
            state.pending.insert(id, tx);

            rx
        };

        // Read responses until we get ours
        // We need to spawn a reader task for this
        self.read_until_response(id).await?;

        let response = rx.await.map_err(|_| anyhow!("Response channel closed"))?;

        if let Some(err) = response.error {
            return Err(anyhow!("RPC error {}: {}", err.code, err.message));
        }

        let result = response
            .result
            .ok_or_else(|| anyhow!("No result in response"))?;

        serde_json::from_value(result).context("Failed to parse response")
    }

    /// Read from stdout until we get the response we're waiting for
    async fn read_until_response(&self, target_id: i64) -> Result<()> {
        loop {
            // Verify process is still running before reading
            {
                let state = self.state.lock().await;
                if state.process.is_none() {
                    return Err(anyhow!("Process not started"));
                }
            }

            // This is a bit hacky - we need to read without holding the lock
            // For now, let's use a simpler approach
            let line = self.read_line().await?;

            if line.is_empty() {
                return Err(anyhow!("EOF from subprocess"));
            }

            debug!(tool = %self.tool.name, line = %line.trim(), "Received line");

            let response: Response = serde_json::from_str(&line)
                .with_context(|| format!("Invalid JSON: {}", line.trim()))?;

            let response_id = match &response.id {
                RequestId::Number(n) => *n,
                RequestId::String(_) => continue, // Skip string IDs
            };

            let mut state = self.state.lock().await;
            if let Some(tx) = state.pending.remove(&response_id) {
                let _ = tx.send(response);
                if response_id == target_id {
                    return Ok(());
                }
            }
        }
    }

    /// Read a single line from stdout
    async fn read_line(&self) -> Result<String> {
        let mut state = self.state.lock().await;
        let process = state
            .process
            .as_mut()
            .ok_or_else(|| anyhow!("Process not started"))?;

        let stdout = process
            .stdout
            .as_mut()
            .ok_or_else(|| anyhow!("No stdout"))?;

        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        reader.read_line(&mut line).await?;

        Ok(line)
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
}

impl Drop for ToolProxy {
    fn drop(&mut self) {
        // Try to kill the process if it's still running
        if let Ok(mut state) = self.state.try_lock() {
            if let Some(mut child) = state.process.take() {
                let _ = child.start_kill();
            }
        }
    }
}
