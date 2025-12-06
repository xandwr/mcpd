//! Aggregating MCP server - combines multiple tool servers into one.

use crate::mcp::{
    CallToolParams, CallToolResult, Content, InitializeResult, ListToolsResult, Notification,
    PROTOCOL_VERSION, Request, RequestId, Response, ServerCapabilities, ServerInfo,
    Tool as McpTool, ToolsCapability,
};
use crate::proxy::ToolProxy;
use crate::registry::Registry;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// Aggregating MCP server
pub struct Server {
    registry: Arc<RwLock<Registry>>,
    proxies: RwLock<HashMap<String, Arc<ToolProxy>>>,
    /// Maps prefixed tool name -> (proxy_name, original_tool_name)
    tool_map: RwLock<HashMap<String, (String, String)>>,
    initialized: RwLock<bool>,
}

impl Server {
    pub fn new(registry: Registry) -> Self {
        Self {
            registry: Arc::new(RwLock::new(registry)),
            proxies: RwLock::new(HashMap::new()),
            tool_map: RwLock::new(HashMap::new()),
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

    /// Handle tools/list request
    async fn handle_list_tools(&self, id: RequestId) -> Response {
        if let Err(e) = self.ensure_proxies().await {
            return Response::error(id, -1, format!("Failed to ensure proxies: {}", e));
        }

        let proxies = self.proxies.read().await;
        let mut all_tools = Vec::new();
        let mut tool_map = self.tool_map.write().await;
        tool_map.clear();

        for (proxy_name, proxy) in proxies.iter() {
            match proxy.list_tools().await {
                Ok(tools) => {
                    for tool in tools {
                        // Prefix tool name with proxy name to avoid collisions
                        let prefixed_name = format!("{}__{}", proxy_name, tool.name);
                        tool_map.insert(
                            prefixed_name.clone(),
                            (proxy_name.clone(), tool.name.clone()),
                        );

                        all_tools.push(McpTool {
                            name: prefixed_name,
                            description: tool.description,
                            input_schema: tool.input_schema,
                        });
                    }
                }
                Err(e) => {
                    warn!(proxy = %proxy_name, error = %e, "Failed to list tools from proxy");
                }
            }
        }

        info!(count = all_tools.len(), "Listed tools from all proxies");

        let result = ListToolsResult { tools: all_tools };
        Response::success(id, serde_json::to_value(result).unwrap())
    }

    /// Handle tools/call request
    async fn handle_call_tool(&self, id: RequestId, params: CallToolParams) -> Response {
        let (proxy_name, original_name) = {
            let tool_map = self.tool_map.read().await;
            match tool_map.get(&params.name) {
                Some((pn, on)) => (pn.clone(), on.clone()),
                None => {
                    return Response::error(id, -1, format!("Unknown tool: {}", params.name));
                }
            }
        };

        let proxy = {
            let proxies = self.proxies.read().await;
            match proxies.get(&proxy_name) {
                Some(p) => p.clone(),
                None => {
                    return Response::error(id, -1, format!("Proxy not found: {}", proxy_name));
                }
            }
        };

        match proxy.call_tool(&original_name, params.arguments).await {
            Ok(result) => Response::success(id, serde_json::to_value(result).unwrap()),
            Err(e) => {
                error!(tool = %params.name, error = %e, "Tool call failed");
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
