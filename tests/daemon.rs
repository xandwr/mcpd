#![cfg(all(feature = "_test", unix))]

use mcpd::protocol;
use mcpd::registry::{Registry, Tool};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::process::{Child, Command};

struct Client {
    input: Lines<BufReader<OwnedReadHalf>>,
    output: OwnedWriteHalf,
    next: u64,
}

impl Client {
    async fn connect(path: &Path) -> Self {
        let stream = UnixStream::connect(path).await.unwrap();
        let (input, output) = stream.into_split();
        Self {
            input: BufReader::new(input).lines(),
            output,
            next: 1,
        }
    }

    async fn send(&mut self, value: Value) {
        self.output
            .write_all(format!("{}\n", value).as_bytes())
            .await
            .unwrap();
        self.output.flush().await.unwrap();
    }

    async fn receive(&mut self) -> Value {
        let line = tokio::time::timeout(std::time::Duration::from_secs(5), self.input.next_line())
            .await
            .expect("Timed out waiting for mcpd daemon")
            .unwrap()
            .expect("mcpd daemon closed the connection");
        serde_json::from_str(&line).unwrap()
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await;
        let response = self.receive().await;
        assert_eq!(response["id"], id);
        response
    }
}

async fn start_daemon(config: &Path, runtime: &Path) -> Child {
    let mut child = Command::new(env!("CARGO_BIN_EXE_mcpd"))
        .arg("daemon")
        .env("XDG_CONFIG_HOME", config)
        .env("XDG_RUNTIME_DIR", runtime)
        .env("RUST_LOG", "off")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let socket = runtime.join("mcpd.sock");
    for _ in 0..500 {
        if std::fs::metadata(&socket)
            .is_ok_and(|metadata| metadata.permissions().mode() & 0o777 == 0o600)
        {
            return child;
        }
        if let Some(status) = child.try_wait().unwrap() {
            panic!("mcpd daemon exited during startup: {status}");
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("mcpd daemon did not create its socket");
}

async fn stop_daemon(mut child: Child) {
    Command::new("kill")
        .arg("-INT")
        .arg(child.id().unwrap().to_string())
        .status()
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), child.wait())
        .await
        .unwrap()
        .unwrap();
}

fn register(registry: &mut Registry, name: &str, log: &Path) {
    registry
        .register(Tool {
            name: name.into(),
            command: vec![env!("CARGO_BIN_EXE_modern-mcp-server").into()],
            env: HashMap::from([("MOCK_LOG".into(), log.to_string_lossy().into_owned())]),
        })
        .unwrap();
}

#[tokio::test]
async fn daemon_shares_backends_and_isolates_client_connections() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config");
    let runtime = directory.path().join("runtime");
    std::fs::create_dir_all(config.join("mcpd")).unwrap();
    std::fs::create_dir_all(&runtime).unwrap();
    let mut registry = Registry::load_from(config.join("mcpd/registry.json")).unwrap();
    let primary_log = directory.path().join("primary.log");
    register(&mut registry, "primary", &primary_log);
    let daemon = start_daemon(&config, &runtime).await;
    let socket = runtime.join("mcpd.sock");
    assert_eq!(
        std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777,
        0o600
    );

    let mut first = Client::connect(&socket).await;
    let mut second = Client::connect(&socket).await;
    let request = || {
        json!({
            "name": "find_tools",
            "arguments": {"server": "primary"},
            "_meta": protocol::metadata()
        })
    };
    let (first_result, second_result) = tokio::join!(
        first.call("tools/call", request()),
        second.call("tools/call", request())
    );
    assert_eq!(first_result["result"]["resultType"], "complete");
    assert_eq!(second_result["result"]["resultType"], "complete");
    let log = std::fs::read_to_string(&primary_log).unwrap();
    assert_eq!(log.matches("server/discover").count(), 1);

    first
        .send(json!({
            "jsonrpc": "2.0",
            "id": "subscription",
            "method": "subscriptions/listen",
            "params": {
                "_meta": protocol::metadata(),
                "notifications": {"resourcesListChanged": true}
            }
        }))
        .await;
    assert_eq!(
        first.receive().await["method"],
        "notifications/subscriptions/acknowledged"
    );
    register(&mut registry, "added", &directory.path().join("added.log"));
    let refreshed = second
        .call("resources/list", json!({"_meta": protocol::metadata()}))
        .await;
    assert_eq!(
        refreshed["result"]["resources"].as_array().unwrap().len(),
        2
    );
    let changed = first.receive().await;
    assert_eq!(changed["method"], "notifications/resources/list_changed");
    assert_eq!(
        changed["params"]["_meta"]["io.modelcontextprotocol/subscriptionId"],
        "subscription"
    );

    drop(first);
    let invoked = second
        .call(
            "tools/call",
            json!({
                "name": "use_tool",
                "arguments": {"tool_name": "primary__echo", "arguments": {"alive": true}},
                "_meta": protocol::metadata()
            }),
        )
        .await;
    assert_eq!(invoked["result"]["structuredContent"]["alive"], true);

    drop(second);
    stop_daemon(daemon).await;
    assert!(!socket.exists());
}
