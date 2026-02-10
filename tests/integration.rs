#![cfg(feature = "_test")]

use mcpd::proxy::ToolProxy;
use mcpd::registry::Tool;
use std::collections::HashMap;

fn mock_tool() -> Tool {
    let mock_path = env!("CARGO_BIN_EXE_mock-mcp-server");
    Tool {
        name: "mock".to_string(),
        command: vec![mock_path.to_string()],
        env: HashMap::new(),
    }
}

#[tokio::test]
async fn proxy_list_tools() {
    let proxy = ToolProxy::new(mock_tool());
    let tools = proxy.list_tools().await.unwrap();
    assert_eq!(tools.len(), 2);
    assert!(tools.iter().any(|t| t.name == "echo"));
    assert!(tools.iter().any(|t| t.name == "fail"));
    proxy.stop().await.unwrap();
}

#[tokio::test]
async fn proxy_call_tool_echo() {
    let proxy = ToolProxy::new(mock_tool());
    let result = proxy
        .call_tool("echo", serde_json::json!({"msg": "hi"}))
        .await
        .unwrap();
    assert!(!result.is_error);
    proxy.stop().await.unwrap();
}

#[tokio::test]
async fn proxy_call_tool_fail() {
    let proxy = ToolProxy::new(mock_tool());
    let result = proxy
        .call_tool("fail", serde_json::json!({}))
        .await
        .unwrap();
    assert!(result.is_error);
    proxy.stop().await.unwrap();
}

#[tokio::test]
async fn proxy_list_resources() {
    let proxy = ToolProxy::new(mock_tool());
    let resources = proxy.list_resources().await.unwrap();
    assert_eq!(resources.len(), 1);
    assert_eq!(resources[0].name, "test_file");
    proxy.stop().await.unwrap();
}

#[tokio::test]
async fn proxy_read_resource() {
    let proxy = ToolProxy::new(mock_tool());
    let result = proxy.read_resource("file:///test.txt").await.unwrap();
    assert_eq!(result.contents.len(), 1);
    assert_eq!(result.contents[0].text.as_deref(), Some("hello world"));
    proxy.stop().await.unwrap();
}

#[tokio::test]
async fn proxy_list_prompts() {
    let proxy = ToolProxy::new(mock_tool());
    let prompts = proxy.list_prompts().await.unwrap();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].name, "greet");
    proxy.stop().await.unwrap();
}

#[tokio::test]
async fn proxy_get_prompt() {
    let proxy = ToolProxy::new(mock_tool());
    let result = proxy
        .get_prompt(
            "greet",
            HashMap::from([("name".to_string(), "World".to_string())]),
        )
        .await
        .unwrap();
    assert_eq!(result.messages.len(), 1);
    assert_eq!(result.messages[0].role, "user");
    proxy.stop().await.unwrap();
}
