#![cfg(feature = "_test")]

use mcpd::mcp::Content;
use mcpd::proxy::ToolProxy;
use mcpd::registry::{Registry, Tool};
use serde_json::{Value, json};
use std::collections::HashMap;

fn decode(result: mcpd::mcp::CallToolResult) -> Value {
    assert!(!result.is_error);
    let Content::Text { text } = &result.content[0] else {
        panic!("Expected text")
    };
    serde_json::from_str(text).unwrap()
}

#[tokio::test]
async fn discovery_over_stdio_handles_filtering_errors_reload_and_invocation() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("mcpd")).unwrap();
    let mut registry = Registry::load_from(dir.path().join("mcpd/registry.json")).unwrap();
    registry
        .register(Tool {
            name: "mock".into(),
            command: vec![env!("CARGO_BIN_EXE_mock-mcp-server").into()],
            env: HashMap::new(),
        })
        .unwrap();
    registry
        .register(Tool {
            name: "broken".into(),
            command: vec![
                dir.path()
                    .join("nonexistent")
                    .to_string_lossy()
                    .into_owned(),
            ],
            env: HashMap::new(),
        })
        .unwrap();
    let proxy = ToolProxy::new(Tool {
        name: "mcpd".into(),
        command: vec![env!("CARGO_BIN_EXE_mcpd").into(), "serve".into()],
        env: HashMap::from([(
            "XDG_CONFIG_HOME".into(),
            dir.path().to_string_lossy().into_owned(),
        )]),
    });
    let tools = proxy.list_tools().await.unwrap();
    assert!(tools.iter().any(|tool| tool.name == "find_tools"));
    assert_eq!(tools.len(), 3);
    let found = decode(
        proxy
            .call_tool("find_tools", json!({"query": "ECHO"}))
            .await
            .unwrap(),
    );
    assert_eq!(found["tools"][0]["name"], "mock__echo");
    assert_eq!(found["total_matches"], 1);
    assert_eq!(found["servers"], json!(["broken", "mock"]));
    assert_eq!(found["errors"][0]["server"], "broken");
    let scoped = decode(
        proxy
            .call_tool("find_tools", json!({"server": "mock", "limit": 1}))
            .await
            .unwrap(),
    );
    assert_eq!(scoped["total_matches"], 2);
    assert_eq!(scoped["tools"].as_array().unwrap().len(), 1);
    assert_eq!(scoped["errors"], json!([]));
    let result = proxy
        .call_tool(
            "use_tool",
            json!({"tool_name": found["tools"][0]["name"], "arguments": {"message": "discovered"}}),
        )
        .await
        .unwrap();
    assert_eq!(decode(result), json!({"message": "discovered"}));
    let empty = decode(
        proxy
            .call_tool(
                "find_tools",
                json!({"query": "unmatched", "server": "mock"}),
            )
            .await
            .unwrap(),
    );
    assert_eq!(empty["total_matches"], 0);
    for arguments in [
        json!({"limit": 0}),
        json!({"limit": 101}),
        json!({"query": 1}),
        json!({"typo": "echo"}),
    ] {
        assert!(proxy.call_tool("find_tools", arguments).await.is_err());
    }
    assert!(
        proxy
            .call_tool("find_tools", json!({"server": "unknown"}))
            .await
            .unwrap()
            .is_error
    );
    registry.unregister("broken").unwrap();
    let refreshed = decode(proxy.call_tool("find_tools", json!({})).await.unwrap());
    assert_eq!(refreshed["servers"], json!(["mock"]));
    assert_eq!(refreshed["errors"], json!([]));
    proxy.stop().await.unwrap();
}
