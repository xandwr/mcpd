use crate::mcp::{LEGACY_PROTOCOL_VERSION, PROTOCOL_VERSION, Request, Response, RpcError};
use serde_json::{Value, json};

pub const VERSION: &str = "io.modelcontextprotocol/protocolVersion";
pub const CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";
pub const CLIENT_INFO: &str = "io.modelcontextprotocol/clientInfo";
pub const SERVER_INFO: &str = "io.modelcontextprotocol/serverInfo";
pub const SUBSCRIPTION_ID: &str = "io.modelcontextprotocol/subscriptionId";

pub fn identity() -> Value {
    json!({"name": "mcpd", "version": env!("CARGO_PKG_VERSION")})
}

pub fn supported_versions() -> Value {
    json!([PROTOCOL_VERSION, LEGACY_PROTOCOL_VERSION])
}

pub fn metadata() -> Value {
    json!({VERSION: PROTOCOL_VERSION, CAPABILITIES: {}, CLIENT_INFO: identity()})
}

pub fn invalid(message: impl Into<String>) -> RpcError {
    RpcError {
        code: -32602,
        message: message.into(),
        data: None,
    }
}

pub fn request_is_modern(request: &Request, legacy: bool) -> Result<bool, RpcError> {
    if request.jsonrpc != "2.0" || request.id == crate::mcp::RequestId::Null {
        return Err(RpcError {
            code: -32600,
            message: "Invalid JSON-RPC request".into(),
            data: None,
        });
    }
    let params = request.params.as_ref();
    if params.is_some_and(|params| !params.is_object()) {
        return Err(invalid("params must be an object"));
    }
    let meta = params.and_then(|params| params.get("_meta"));
    if request.method == "initialize" && meta.is_none() {
        return Ok(false);
    }
    if legacy
        && request.method != "server/discover"
        && meta.is_none_or(|meta| meta.get(VERSION).is_none())
    {
        return Ok(false);
    }
    let meta = meta
        .and_then(Value::as_object)
        .ok_or_else(|| invalid("Missing required request _meta"))?;
    let version = meta
        .get(VERSION)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("Missing protocol version in _meta"))?;
    if version != PROTOCOL_VERSION {
        return Err(RpcError {
            code: -32022,
            message: "Unsupported protocol version".into(),
            data: Some(json!({"supported": supported_versions(), "requested": version})),
        });
    }
    if !meta.get(CAPABILITIES).is_some_and(Value::is_object) {
        return Err(invalid("Missing client capabilities object in _meta"));
    }
    if let Some(info) = meta.get(CLIENT_INFO)
        && (!info.get("name").is_some_and(Value::is_string)
            || !info.get("version").is_some_and(Value::is_string))
    {
        return Err(invalid("clientInfo must contain name and version strings"));
    }
    if let Some(params) = params {
        if params
            .get("requestState")
            .is_some_and(|state| !state.is_string())
        {
            return Err(invalid("requestState must be a string"));
        }
        if params
            .get("inputResponses")
            .is_some_and(|inputs| !inputs.is_object())
        {
            return Err(invalid("inputResponses must be an object"));
        }
    }
    Ok(true)
}

pub fn finish(response: &mut Response, method: &str, modern: bool) {
    let Some(result) = response.result.as_mut().and_then(Value::as_object_mut) else {
        return;
    };
    if modern {
        result.entry("resultType").or_insert(json!("complete"));
        let meta = result.entry("_meta").or_insert(json!({}));
        if let Some(meta) = meta.as_object_mut() {
            meta.insert(SERVER_INFO.into(), identity());
        }
        if result.get("resultType") == Some(&json!("complete"))
            && matches!(
                method,
                "server/discover"
                    | "tools/list"
                    | "resources/list"
                    | "resources/templates/list"
                    | "resources/read"
                    | "prompts/list"
            )
        {
            result.insert("ttlMs".into(), json!(0));
            result.insert("cacheScope".into(), json!("private"));
        }
    } else {
        result.remove("resultType");
        result.remove("ttlMs");
        result.remove("cacheScope");
    }
}

pub fn error_response(id: crate::mcp::RequestId, error: RpcError) -> Response {
    Response {
        jsonrpc: "2.0".into(),
        id,
        result: None,
        error: Some(error),
    }
}
