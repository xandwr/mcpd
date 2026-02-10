//! MCP protocol types and JSON-RPC handling.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// JSON-RPC request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    pub id: RequestId,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// JSON-RPC notification (no id, no response expected)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// JSON-RPC response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: String,
    pub id: RequestId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

/// JSON-RPC error
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// Request ID can be string or number
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(untagged)]
pub enum RequestId {
    Number(i64),
    String(String),
}

impl Request {
    pub fn new(id: impl Into<RequestId>, method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id: id.into(),
            method: method.into(),
            params,
        }
    }
}

impl Notification {
    pub fn new(method: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            method: method.into(),
            params: None,
        }
    }
}

impl Response {
    pub fn success(id: RequestId, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: RequestId, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

impl From<i64> for RequestId {
    fn from(n: i64) -> Self {
        RequestId::Number(n)
    }
}

impl From<String> for RequestId {
    fn from(s: String) -> Self {
        RequestId::String(s)
    }
}

// MCP-specific types

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub protocol_version: String,
    pub capabilities: ClientCapabilities,
    pub client_info: ClientInfo,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClientCapabilities {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: String,
    pub capabilities: ServerCapabilities,
    pub server_info: ServerInfo,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerCapabilities {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolsCapability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourcesCapability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompts: Option<PromptsCapability>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolsCapability {
    #[serde(default)]
    pub list_changed: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourcesCapability {
    #[serde(default)]
    pub list_changed: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptsCapability {
    #[serde(default)]
    pub list_changed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub input_schema: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListToolsResult {
    pub tools: Vec<Tool>,
}

// Resource types

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resource {
    pub uri: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListResourcesResult {
    pub resources: Vec<Resource>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadResourceParams {
    pub uri: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadResourceResult {
    pub contents: Vec<ResourceContent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceContent {
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob: Option<String>,
}

// Prompt types

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prompt {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arguments: Vec<PromptArgument>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptArgument {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListPromptsResult {
    pub prompts: Vec<Prompt>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetPromptParams {
    pub name: String,
    #[serde(default)]
    pub arguments: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetPromptResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub messages: Vec<PromptMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptMessage {
    pub role: String,
    pub content: PromptContent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum PromptContent {
    Text { text: String },
    Image { data: String, mime_type: String },
    Resource { resource: ResourceContent },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallToolParams {
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallToolResult {
    pub content: Vec<Content>,
    #[serde(default)]
    pub is_error: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Content {
    Text { text: String },
    Image { data: String, mime_type: String },
    Resource { resource: Value },
}

/// Protocol version we support
pub const PROTOCOL_VERSION: &str = "2025-11-25";

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_new_with_number_id() {
        let req = Request::new(1_i64, "tools/list", None);
        assert_eq!(req.jsonrpc, "2.0");
        assert_eq!(req.id, RequestId::Number(1));
        assert_eq!(req.method, "tools/list");
        assert!(req.params.is_none());
    }

    #[test]
    fn request_new_with_string_id() {
        let req = Request::new("abc".to_string(), "initialize", Some(json!({"key": "val"})));
        assert_eq!(req.id, RequestId::String("abc".to_string()));
        assert!(req.params.is_some());
    }

    #[test]
    fn request_json_roundtrip() {
        let req = Request::new(42_i64, "tools/call", Some(json!({"name": "test"})));
        let json_str = serde_json::to_string(&req).unwrap();
        let parsed: Request = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed.method, "tools/call");
        assert_eq!(parsed.id, RequestId::Number(42));
    }

    #[test]
    fn response_success_roundtrip() {
        let resp = Response::success(RequestId::Number(1), json!({"tools": []}));
        let json_str = serde_json::to_string(&resp).unwrap();
        let parsed: Response = serde_json::from_str(&json_str).unwrap();
        assert!(parsed.result.is_some());
        assert!(parsed.error.is_none());
    }

    #[test]
    fn response_error_roundtrip() {
        let resp = Response::error(RequestId::Number(1), -32601, "Method not found");
        let json_str = serde_json::to_string(&resp).unwrap();
        let parsed: Response = serde_json::from_str(&json_str).unwrap();
        assert!(parsed.result.is_none());
        let err = parsed.error.unwrap();
        assert_eq!(err.code, -32601);
        assert_eq!(err.message, "Method not found");
    }

    #[test]
    fn notification_new() {
        let n = Notification::new("notifications/initialized");
        assert_eq!(n.jsonrpc, "2.0");
        assert_eq!(n.method, "notifications/initialized");
        assert!(n.params.is_none());
    }

    #[test]
    fn request_id_number_serde() {
        let id = RequestId::Number(42);
        let json_str = serde_json::to_string(&id).unwrap();
        assert_eq!(json_str, "42");
        let parsed: RequestId = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed, id);
    }

    #[test]
    fn request_id_string_serde() {
        let id = RequestId::String("req-001".to_string());
        let json_str = serde_json::to_string(&id).unwrap();
        assert_eq!(json_str, "\"req-001\"");
        let parsed: RequestId = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed, id);
    }

    #[test]
    fn initialize_result_camel_case() {
        let result = InitializeResult {
            protocol_version: "2025-11-25".to_string(),
            capabilities: ServerCapabilities::default(),
            server_info: ServerInfo {
                name: "test".to_string(),
                version: "0.1.0".to_string(),
            },
        };
        let json_val = serde_json::to_value(&result).unwrap();
        assert!(json_val.get("protocolVersion").is_some());
        assert!(json_val.get("serverInfo").is_some());
    }

    #[test]
    fn call_tool_params_roundtrip() {
        let json_str = r#"{"name":"my_tool","arguments":{"path":"/tmp"}}"#;
        let params: CallToolParams = serde_json::from_str(json_str).unwrap();
        assert_eq!(params.name, "my_tool");
        assert_eq!(params.arguments["path"], "/tmp");
    }

    #[test]
    fn content_tagged_enum() {
        let text = Content::Text {
            text: "hello".to_string(),
        };
        let json_val = serde_json::to_value(&text).unwrap();
        assert_eq!(json_val["type"], "text");
        assert_eq!(json_val["text"], "hello");
    }

    #[test]
    fn prompt_content_tagged_enum() {
        let text = PromptContent::Text {
            text: "hello".to_string(),
        };
        let json_val = serde_json::to_value(&text).unwrap();
        assert_eq!(json_val["type"], "text");
        assert_eq!(json_val["text"], "hello");
    }

    #[test]
    fn server_capabilities_default_skips_none() {
        let caps = ServerCapabilities::default();
        let json_val = serde_json::to_value(&caps).unwrap();
        assert_eq!(json_val, json!({}));
    }

    #[test]
    fn resource_optional_fields_skip() {
        let r = Resource {
            uri: "file:///test".to_string(),
            name: "test".to_string(),
            description: None,
            mime_type: None,
        };
        let json_val = serde_json::to_value(&r).unwrap();
        assert!(json_val.get("description").is_none());
        assert!(json_val.get("mimeType").is_none());
    }

    #[test]
    fn call_tool_result_is_error_defaults_false() {
        let json_str = r#"{"content":[{"type":"text","text":"ok"}]}"#;
        let result: CallToolResult = serde_json::from_str(json_str).unwrap();
        assert!(!result.is_error);
        assert_eq!(result.content.len(), 1);
    }
}
