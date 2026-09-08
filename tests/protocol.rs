#![cfg(feature = "_test")]

use mcpd::mcp::RpcError;
use mcpd::protocol::{self, CAPABILITIES, SERVER_INFO, SUBSCRIPTION_ID, VERSION};
use mcpd::proxy::ToolProxy;
use mcpd::registry::{Registry, Tool};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

struct Client {
    child: Child,
    daemon: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    directory: tempfile::TempDir,
    registry: Registry,
    next: u64,
}

impl Client {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("mcpd")).unwrap();
        let runtime = directory.path().join("runtime");
        std::fs::create_dir(&runtime).unwrap();
        let registry = Registry::load_from(directory.path().join("mcpd/registry.json")).unwrap();
        let mut daemon = Command::new(env!("CARGO_BIN_EXE_mcpd"))
            .arg("daemon")
            .env("XDG_CONFIG_HOME", directory.path())
            .env("XDG_RUNTIME_DIR", &runtime)
            .env("RUST_LOG", "off")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let socket = runtime.join("mcpd.sock");
        for _ in 0..500 {
            if socket.exists() {
                break;
            }
            assert!(daemon.try_wait().unwrap().is_none());
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(socket.exists());
        let mut child = Command::new(env!("CARGO_BIN_EXE_mcpd"))
            .arg("serve")
            .env("XDG_CONFIG_HOME", directory.path())
            .env("XDG_RUNTIME_DIR", runtime)
            .env("RUST_LOG", "off")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let lines = BufReader::new(child.stdout.take().unwrap()).lines();
        Self {
            child,
            daemon,
            stdin,
            lines,
            directory,
            registry,
            next: 1,
        }
    }

    fn register(&mut self, name: &str, modern: bool) {
        self.registry
            .register(Tool {
                name: name.into(),
                command: vec![
                    if modern {
                        env!("CARGO_BIN_EXE_modern-mcp-server")
                    } else {
                        env!("CARGO_BIN_EXE_mock-mcp-server")
                    }
                    .into(),
                ],
                env: HashMap::from([(
                    "MOCK_LOG".into(),
                    self.directory
                        .path()
                        .join(format!("{}.log", name))
                        .to_string_lossy()
                        .into_owned(),
                )]),
            })
            .unwrap();
    }

    async fn send(&mut self, value: Value) {
        self.stdin
            .write_all(format!("{}\n", value).as_bytes())
            .await
            .unwrap();
        self.stdin.flush().await.unwrap();
    }

    async fn receive(&mut self) -> Value {
        let line = tokio::time::timeout(std::time::Duration::from_secs(5), self.lines.next_line())
            .await
            .expect("Timed out waiting for mcpd")
            .unwrap()
            .expect("mcpd exited");
        serde_json::from_str(&line).unwrap()
    }

    async fn call(&mut self, method: &str, mut params: Value) -> Value {
        params
            .as_object_mut()
            .unwrap()
            .entry("_meta")
            .or_insert(protocol::metadata());
        let id = self.next;
        self.next += 1;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await;
        let message = self.receive().await;
        assert_eq!(
            message.get("id"),
            Some(&json!(id)),
            "Unexpected message: {}",
            message
        );
        message
    }

    fn log(&self, name: &str) -> Vec<Value> {
        std::fs::read_to_string(self.directory.path().join(format!("{}.log", name)))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    async fn stop(mut self) {
        self.stdin.shutdown().await.unwrap();
        drop(self.stdin);
        tokio::time::timeout(std::time::Duration::from_secs(3), self.child.wait())
            .await
            .unwrap()
            .unwrap();
        Command::new("kill")
            .arg("-INT")
            .arg(self.daemon.id().unwrap().to_string())
            .status()
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), self.daemon.wait())
            .await
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn stateless_discovery_version_errors_and_cache_fields() {
    let mut client = Client::new().await;
    let first = client.call("tools/list", json!({})).await;
    assert_eq!(first["result"]["resultType"], "complete");
    assert_eq!(first["result"]["ttlMs"], 0);
    assert_eq!(first["result"]["cacheScope"], "private");
    assert_eq!(first["result"]["_meta"][SERVER_INFO]["name"], "mcpd");
    let discover = client.call("server/discover", json!({})).await;
    assert_eq!(
        discover["result"]["supportedVersions"],
        json!(["2026-07-28", "2025-11-25"])
    );
    let unsupported = client
        .call(
            "tools/list",
            json!({"_meta": {VERSION: "2099-01-01", CAPABILITIES: {}}}),
        )
        .await;
    assert_eq!(unsupported["error"]["code"], -32022);
    assert_eq!(unsupported["error"]["data"]["requested"], "2099-01-01");
    assert_eq!(
        unsupported["error"]["data"]["supported"],
        discover["result"]["supportedVersions"]
    );
    let missing = client
        .call("tools/list", json!({"_meta": {VERSION: "2026-07-28"}}))
        .await;
    assert_eq!(missing["error"]["code"], -32602);
    assert_eq!(
        client.call("ping", json!({})).await["error"]["code"],
        -32601
    );
    client
        .send(json!({"jsonrpc": "2.0", "id": 1000, "method": "tools/list"}))
        .await;
    assert_eq!(client.receive().await["error"]["code"], -32602);
    client.stdin.write_all(b"bad-json\n").await.unwrap();
    let invalid = client.receive().await;
    assert_eq!(invalid["id"], Value::Null);
    assert_eq!(invalid["error"]["code"], -32700);
    client.stop().await;
}

#[tokio::test]
async fn modern_and_legacy_backends_preserve_catalogs_and_rich_results() {
    let mut client = Client::new().await;
    client.register("modern", true);
    client.register("legacy", false);
    let found = client
        .call(
            "tools/call",
            json!({"name": "find_tools", "arguments": {"query": "modern"}}),
        )
        .await;
    let found: Value =
        serde_json::from_str(found["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(found["total_matches"], 2);
    assert_eq!(
        found["tools"][0]["outputSchema"],
        json!({"anyOf": [{"type": "array"}, {"type": "null"}]})
    );
    assert_eq!(found["tools"][0]["annotations"]["readOnlyHint"], true);
    assert_eq!(found["tools"][0]["_meta"]["example/label"], "preserved");
    let log = client.log("modern");
    assert_eq!(log[0]["method"], "server/discover");
    assert!(!log.iter().any(|request| request["method"] == "initialize"));
    assert!(
        log.iter()
            .any(|request| request["params"]["cursor"] == "page-2")
    );
    for (method, field) in [
        ("resources/list", "resources"),
        ("resources/templates/list", "resourceTemplates"),
        ("prompts/list", "prompts"),
    ] {
        let response = client.call(method, json!({})).await;
        assert_eq!(response["result"]["ttlMs"], 0);
        assert_eq!(response["result"]["cacheScope"], "private");
        assert!(!response["result"][field].as_array().unwrap().is_empty());
    }
    let rich = client
        .call(
            "tools/call",
            json!({"name": "use_tool", "arguments": {"tool_name": "modern__rich"}}),
        )
        .await;
    assert_eq!(rich["result"]["structuredContent"], json!([1, "two", null]));
    assert_eq!(
        rich["result"]["content"][0]["annotations"]["audience"],
        json!(["user"])
    );
    assert_eq!(rich["result"]["content"][1]["mimeType"], "image/png");
    assert_eq!(rich["result"]["content"][2]["type"], "audio");
    assert_eq!(rich["result"]["content"][3]["type"], "resource_link");
    assert_eq!(rich["result"]["_meta"]["example/result"], true);
    let resource = client
        .call(
            "resources/read",
            json!({"uri": "mcpd://modern/file:///readme"}),
        )
        .await;
    assert_eq!(
        resource["result"]["contents"][0]["uri"],
        "mcpd://modern/file:///readme"
    );
    assert_eq!(
        resource["result"]["contents"][0]["_meta"]["example/content"],
        true
    );
    let legacy = client.call("tools/call", json!({"name": "use_tool", "arguments": {"tool_name": "legacy__echo", "arguments": {"hello": "old server"}}})).await;
    assert_eq!(legacy["result"]["resultType"], "complete");
    assert_eq!(legacy["result"]["isError"], false);
    client.stop().await;
}

#[tokio::test]
async fn changed_backend_spec_replaces_the_live_proxy() {
    let mut client = Client::new().await;
    client.register("swap", false);
    let first = client
        .call(
            "tools/call",
            json!({"name": "find_tools", "arguments": {"query": "echo", "server": "swap"}}),
        )
        .await;
    let first: Value =
        serde_json::from_str(first["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(first["tools"][0]["name"], "swap__echo");

    client.register("swap", true);
    let replaced = client
        .call(
            "tools/call",
            json!({"name": "find_tools", "arguments": {"query": "needs_input", "server": "swap"}}),
        )
        .await;
    let replaced: Value =
        serde_json::from_str(replaced["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(replaced["tools"][0]["name"], "swap__needs_input");
    client.stop().await;
}

#[tokio::test]
async fn mrtr_retries_preserve_state_capabilities_and_backend_errors() {
    let mut client = Client::new().await;
    client.register("modern", true);
    for (method, params) in [
        (
            "tools/call",
            json!({"name": "use_tool", "arguments": {"tool_name": "modern__needs_input"}}),
        ),
        (
            "resources/read",
            json!({"uri": "mcpd://modern/file:///needs-input"}),
        ),
        ("prompts/get", json!({"name": "modern__needs_input"})),
    ] {
        let mut params = params;
        params["_meta"] = protocol::metadata();
        params["_meta"][CAPABILITIES] = json!({"elicitation": {"form": {}}});
        params["_meta"]["traceparent"] = json!("00-trace-parent");
        let interim = client.call(method, params.clone()).await;
        assert_eq!(interim["result"]["resultType"], "input_required");
        assert!(interim["result"].get("ttlMs").is_none());
        params["requestState"] = interim["result"]["requestState"].clone();
        params["inputResponses"] =
            json!({"answer": {"action": "accept", "content": {"name": "test"}}});
        let complete = client.call(method, params).await;
        assert_eq!(complete["result"]["resultType"], "complete");
        let forwarded = &complete["result"]["structuredContent"]["params"];
        assert_eq!(forwarded["requestState"], "opaque:do-not-parse");
        assert_eq!(forwarded["inputResponses"]["answer"]["action"], "accept");
        assert_eq!(
            forwarded["_meta"][CAPABILITIES]["elicitation"],
            json!({"form": {}})
        );
        assert_eq!(forwarded["_meta"]["traceparent"], "00-trace-parent");
    }
    let missing = client
        .call(
            "tools/call",
            json!({"name": "use_tool", "arguments": {"tool_name": "modern__needs_input"}}),
        )
        .await;
    assert_eq!(missing["error"]["code"], -32021);
    let rpc = client
        .call(
            "tools/call",
            json!({"name": "use_tool", "arguments": {"tool_name": "modern__rpc_error"}}),
        )
        .await;
    assert_eq!(
        rpc["error"]["data"]["requiredCapabilities"],
        json!({"elicitation": {"form": {}}})
    );
    client.stop().await;
}

#[tokio::test]
async fn subscriptions_are_opt_in_filtered_and_cancellable() {
    let mut client = Client::new().await;
    client.send(json!({"jsonrpc": "2.0", "id": "subscription", "method": "subscriptions/listen", "params": {
        "_meta": protocol::metadata(), "notifications": {"resourcesListChanged": true, "resourceSubscriptions": ["file:///not-supported"]}
    }})).await;
    let acknowledged = client.receive().await;
    assert_eq!(
        acknowledged["method"],
        "notifications/subscriptions/acknowledged"
    );
    assert_eq!(
        acknowledged["params"]["_meta"][SUBSCRIPTION_ID],
        "subscription"
    );
    assert_eq!(
        acknowledged["params"]["notifications"],
        json!({"resourcesListChanged": true})
    );
    client.register("modern", true);
    client.send(json!({"jsonrpc": "2.0", "id": 100, "method": "resources/list", "params": {"_meta": protocol::metadata()}})).await;
    let changed = client.receive().await;
    assert_eq!(changed["method"], "notifications/resources/list_changed");
    assert_eq!(changed["params"]["_meta"][SUBSCRIPTION_ID], "subscription");
    assert_eq!(client.receive().await["id"], 100);
    client.send(json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": "subscription"}})).await;
    client.registry.unregister("modern").unwrap();
    let response = client.call("resources/list", json!({})).await;
    assert_eq!(response["result"]["resources"], json!([]));
    client.stop().await;
}

#[tokio::test]
async fn cancellation_reaches_backend_and_does_not_block_other_requests() {
    let mut client = Client::new().await;
    client.register("modern", true);
    client.send(json!({"jsonrpc": "2.0", "id": 100, "method": "tools/call", "params": {
        "_meta": protocol::metadata(), "name": "use_tool", "arguments": {"tool_name": "modern__slow"}
    }})).await;
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while !client
            .log("modern")
            .iter()
            .any(|request| request["method"] == "tools/call")
        {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    client.send(json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 100}})).await;
    assert!(
        client
            .call("tools/list", json!({}))
            .await
            .get("result")
            .is_some()
    );
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while !client
            .log("modern")
            .iter()
            .any(|request| request["method"] == "notifications/cancelled")
        {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            client.lines.next_line()
        )
        .await
        .is_err()
    );
    client.stop().await;
}

#[tokio::test]
async fn legacy_clients_can_still_initialize() {
    let mut client = Client::new().await;
    client.register("legacy", false);
    client.send(json!({"jsonrpc": "2.0", "id": 100, "method": "initialize", "params": {
        "protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "old-client", "version": "1"}
    }})).await;
    let initialize = client.receive().await;
    assert_eq!(initialize["result"]["protocolVersion"], "2025-11-25");
    assert!(initialize["result"].get("resultType").is_none());
    client
        .send(json!({"jsonrpc": "2.0", "id": 101, "method": "tools/list"}))
        .await;
    assert_eq!(
        client.receive().await["result"]["tools"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    client.stop().await;
}

#[tokio::test]
async fn backend_probe_falls_back_on_legacy_errors_and_silence_but_not_modern_errors() {
    for mode in ["legacy-invalid", "legacy-silent", "unsupported"] {
        let directory = tempfile::tempdir().unwrap();
        let log = directory.path().join("requests.jsonl");
        let proxy = ToolProxy::new(Tool {
            name: "probe".into(),
            command: vec![env!("CARGO_BIN_EXE_modern-mcp-server").into()],
            env: HashMap::from([
                ("MOCK_PROTOCOL_MODE".into(), mode.into()),
                ("MOCK_LOG".into(), log.to_string_lossy().into_owned()),
            ]),
        });
        let result = proxy.list_tools().await;
        let messages: Vec<Value> = std::fs::read_to_string(log)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        if mode == "unsupported" {
            assert_eq!(
                result.unwrap_err().downcast_ref::<RpcError>().unwrap().code,
                -32022
            );
            assert!(
                !messages
                    .iter()
                    .any(|message| message["method"] == "initialize")
            );
        } else {
            assert_eq!(result.unwrap().len(), 2);
            assert!(
                messages
                    .iter()
                    .any(|message| message["method"] == "initialize")
            );
        }
        proxy.stop().await.unwrap();
    }
}
