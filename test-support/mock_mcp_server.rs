//! Minimal MCP server for integration testing.
//! Speaks JSON-RPC over stdio. Handles the core MCP methods.

use std::io::{self, BufRead, Write};

fn main() {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };

        let msg: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // Notifications have no "id" field — ignore them
        if msg.get("id").is_none() {
            continue;
        }

        let id = msg["id"].clone();
        let method = msg["method"].as_str().unwrap_or("");

        let response = match method {
            "initialize" => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {
                        "tools": {"listChanged": false},
                        "resources": {"listChanged": false},
                        "prompts": {"listChanged": false}
                    },
                    "serverInfo": {"name": "mock-mcp", "version": "0.1.0"}
                }
            }),
            "tools/list" => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "tools": [
                        {
                            "name": "echo",
                            "description": "Echo back arguments",
                            "inputSchema": {"type": "object"}
                        },
                        {
                            "name": "fail",
                            "description": "Always fails",
                            "inputSchema": {"type": "object"}
                        }
                    ]
                }
            }),
            "tools/call" => {
                let name = msg["params"]["name"].as_str().unwrap_or("");
                if name == "fail" {
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "content": [{"type": "text", "text": "intentional failure"}],
                            "is_error": true
                        }
                    })
                } else {
                    let args = &msg["params"]["arguments"];
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "content": [{"type": "text", "text": serde_json::to_string(args).unwrap()}],
                            "is_error": false
                        }
                    })
                }
            }
            "resources/list" => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "resources": [{
                        "uri": "file:///test.txt",
                        "name": "test_file",
                        "description": "A test file"
                    }]
                }
            }),
            "resources/read" => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "contents": [{
                        "uri": "file:///test.txt",
                        "text": "hello world"
                    }]
                }
            }),
            "prompts/list" => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "prompts": [{
                        "name": "greet",
                        "description": "A greeting prompt",
                        "arguments": [{"name": "name", "required": true}]
                    }]
                }
            }),
            "prompts/get" => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "messages": [{
                        "role": "user",
                        "content": {"type": "text", "text": "Hello!"}
                    }]
                }
            }),
            _ => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32601, "message": "Method not found"}
            }),
        };

        writeln!(out, "{}", serde_json::to_string(&response).unwrap()).unwrap();
        out.flush().unwrap();
    }
}
