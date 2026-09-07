use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

async fn write(out: &Mutex<tokio::io::Stdout>, value: Value) {
    let mut out = out.lock().await;
    out.write_all(format!("{}\n", value).as_bytes())
        .await
        .unwrap();
    out.flush().await.unwrap();
}

fn result(method: &str, params: &Value) -> Value {
    match method {
        "server/discover" => {
            json!({"supportedVersions": ["2026-07-28"], "capabilities": {"tools": {}, "resources": {}, "prompts": {}}})
        }
        "tools/list" => {
            if params["cursor"] == "page-2" {
                json!({"tools": [{"name": "needs_input", "description": "Request extra input", "inputSchema": {"type": "object"}}]})
            } else {
                json!({"nextCursor": "page-2", "tools": [{"name": "echo", "description": "Echo arguments", "title": "Echo", "inputSchema": {"anyOf": [{"type": "object"}]}, "outputSchema": {"anyOf": [{"type": "array"}, {"type": "null"}]}, "annotations": {"readOnlyHint": true}, "_meta": {"example/label": "preserved"}}]})
            }
        }
        "resources/list" => {
            json!({"resources": [{"name": "readme", "uri": "file:///readme", "title": "Read me", "annotations": {"priority": 0.5}}]})
        }
        "resources/templates/list" => {
            json!({"resourceTemplates": [{"name": "files", "uriTemplate": "file:///{path}", "title": "Files"}]})
        }
        "prompts/list" => json!({"prompts": [{"name": "greet", "title": "Greeting"}]}),
        _ if params["name"] == "needs_input" || params["uri"] == "file:///needs-input" => {
            if params.get("requestState").is_none() {
                json!({"resultType": "input_required", "inputRequests": {"answer": {"method": "elicitation/create", "params": {"mode": "form", "message": "Name?", "requestedSchema": {"type": "object"}}}}, "requestState": "opaque:do-not-parse"})
            } else {
                json!({"content": [{"type": "text", "text": "continued"}], "messages": [{"role": "user", "content": {"type": "text", "text": "continued"}}], "contents": [{"uri": "file:///needs-input", "text": "continued"}], "structuredContent": {"params": params}})
            }
        }
        "tools/call" if params["name"] == "retry" && params.get("requestState").is_none() => {
            json!({"resultType": "input_required", "requestState": "retry-state"})
        }
        "tools/call" if params["name"] == "rich" => json!({"content": [
            {"type": "text", "text": "annotated", "annotations": {"audience": ["user"]}, "_meta": {"example/key": 1}},
            {"type": "image", "data": "AA==", "mimeType": "image/png"},
            {"type": "audio", "data": "AA==", "mimeType": "audio/wav"},
            {"type": "resource_link", "name": "file", "uri": "file:///readme"}
        ], "structuredContent": [1, "two", null], "_meta": {"example/result": true}}),
        "tools/call" => {
            json!({"content": [{"type": "text", "text": params["arguments"].to_string()}], "structuredContent": params.get("arguments").cloned().unwrap_or(Value::Null)})
        }
        "resources/read" => {
            json!({"contents": [{"uri": params["uri"], "text": "hello", "_meta": {"example/content": true}}]})
        }
        "prompts/get" => {
            json!({"messages": [{"role": "user", "content": {"type": "audio", "data": "AA==", "mimeType": "audio/wav"}}], "_meta": {"example/prompt": true}})
        }
        _ => json!({}),
    }
}

#[tokio::main]
async fn main() {
    let mode = std::env::var("MOCK_PROTOCOL_MODE").unwrap_or_default();
    let modern = !mode.starts_with("legacy");
    let mut initialized = false;
    let out = Arc::new(Mutex::new(tokio::io::stdout()));
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut tasks = HashMap::new();
    while let Ok(Some(line)) = lines.next_line().await {
        let message: Value = serde_json::from_str(&line).unwrap();
        if let Ok(path) = std::env::var("MOCK_LOG") {
            let mut log = std::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(path)
                .unwrap();
            writeln!(log, "{}", message).unwrap();
        }
        let method = message["method"].as_str().unwrap_or("");
        if message.get("id").is_none() {
            if method == "notifications/cancelled"
                && let Some(task) = tasks.remove(&message["params"]["requestId"].to_string())
            {
                let task: tokio::task::JoinHandle<()> = task;
                task.abort();
            }
            continue;
        }
        let id = message["id"].clone();
        let params = message["params"].clone();
        let error = if modern {
            if mode == "unsupported" {
                Some(
                    json!({"code": -32022, "message": "Unsupported protocol version", "data": {"supported": ["2099-01-01"], "requested": params["_meta"]["io.modelcontextprotocol/protocolVersion"]}}),
                )
            } else if method == "initialize" {
                Some(json!({"code": -32601, "message": "No initialization in modern MCP"}))
            } else if params["_meta"]["io.modelcontextprotocol/protocolVersion"] != "2026-07-28"
                || !params["_meta"]["io.modelcontextprotocol/clientCapabilities"].is_object()
            {
                Some(json!({"code": -32602, "message": "Missing modern metadata"}))
            } else if params["name"] == "rpc_error" {
                Some(
                    json!({"code": -32021, "message": "Capability required", "data": {"requiredCapabilities": {"elicitation": {"form": {}}}}}),
                )
            } else {
                None
            }
        } else if method == "initialize" {
            initialized = true;
            write(&out, json!({"jsonrpc": "2.0", "id": id, "result": {"protocolVersion": "2025-11-25", "capabilities": {"tools": {}}, "serverInfo": {"name": "legacy", "version": "1"}}})).await;
            continue;
        } else if !initialized {
            if mode == "legacy-silent" {
                continue;
            }
            Some(json!({"code": -32602, "message": "Not initialized"}))
        } else {
            None
        };
        if let Some(error) = error {
            write(&out, json!({"jsonrpc": "2.0", "id": id, "error": error})).await;
            continue;
        }
        let mut value = result(method, &params);
        if modern {
            value
                .as_object_mut()
                .unwrap()
                .entry("resultType")
                .or_insert(json!("complete"));
            if value["resultType"] == "complete" {
                value["ttlMs"] = json!(5000);
                value["cacheScope"] = json!("public");
            }
        }
        let delay = params["name"] == "slow";
        let out = Arc::clone(&out);
        tasks.insert(
            id.to_string(),
            tokio::spawn(async move {
                if delay {
                    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                }
                write(&out, json!({"jsonrpc": "2.0", "id": id, "result": value})).await;
            }),
        );
    }
}
