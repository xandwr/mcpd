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
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

struct Client {
    input: Lines<BufReader<OwnedReadHalf>>,
    output: OwnedWriteHalf,
    next: u64,
}

struct BridgeClient {
    child: Child,
    input: Lines<BufReader<ChildStdout>>,
    output: ChildStdin,
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

impl BridgeClient {
    fn start(config: &Path, runtime: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_mcpd"))
            .arg("serve")
            .env("XDG_CONFIG_HOME", config)
            .env("XDG_RUNTIME_DIR", runtime)
            .env("RUST_LOG", "off")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let output = child.stdin.take().unwrap();
        let input = BufReader::new(child.stdout.take().unwrap()).lines();
        Self {
            child,
            input,
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
            .expect("Timed out waiting for mcpd serve")
            .unwrap()
            .expect("mcpd serve closed stdout");
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

    async fn close(mut self) {
        drop(self.output);
        tokio::time::timeout(std::time::Duration::from_secs(5), self.child.wait())
            .await
            .unwrap()
            .unwrap();
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

#[cfg(target_os = "linux")]
fn daemon_pid(runtime: &Path) -> Option<u32> {
    let expected = format!("XDG_RUNTIME_DIR={}", runtime.display()).into_bytes();
    for entry in std::fs::read_dir("/proc").unwrap().flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(environment) = std::fs::read(entry.path().join("environ")) else {
            continue;
        };
        if !environment
            .split(|byte| *byte == 0)
            .any(|item| item == expected)
        {
            continue;
        }
        let Ok(command) = std::fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        if command
            .split(|byte| *byte == 0)
            .any(|argument| argument == b"daemon")
        {
            return Some(pid);
        }
    }
    None
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

#[tokio::test]
async fn serve_bridges_existing_client_connections_to_the_shared_daemon() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config");
    let runtime = directory.path().join("runtime");
    std::fs::create_dir_all(config.join("mcpd")).unwrap();
    std::fs::create_dir_all(&runtime).unwrap();
    let mut registry = Registry::load_from(config.join("mcpd/registry.json")).unwrap();
    let log = directory.path().join("shared.log");
    register(&mut registry, "shared", &log);
    let daemon = start_daemon(&config, &runtime).await;

    let mut codex = BridgeClient::start(&config, &runtime);
    let mut claude = BridgeClient::start(&config, &runtime);
    let mut pi = BridgeClient::start(&config, &runtime);
    let request = || {
        json!({
            "name": "find_tools",
            "arguments": {"server": "shared"},
            "_meta": protocol::metadata()
        })
    };
    let (codex_result, claude_result, pi_result) = tokio::join!(
        codex.call("tools/call", request()),
        claude.call("tools/call", request()),
        pi.call("tools/call", request())
    );
    assert_eq!(codex_result["result"]["resultType"], "complete");
    assert_eq!(claude_result["result"]["resultType"], "complete");
    assert_eq!(pi_result["result"]["resultType"], "complete");
    assert_eq!(
        std::fs::read_to_string(log)
            .unwrap()
            .matches("server/discover")
            .count(),
        1
    );

    codex.close().await;
    claude.close().await;
    pi.close().await;
    stop_daemon(daemon).await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn concurrent_serve_startup_recovers_a_stale_socket() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config");
    let runtime = directory.path().join("runtime");
    std::fs::create_dir_all(config.join("mcpd")).unwrap();
    std::fs::create_dir_all(&runtime).unwrap();
    let stale = runtime.join("mcpd.sock");
    drop(tokio::net::UnixListener::bind(&stale).unwrap());
    let mut registry = Registry::load_from(config.join("mcpd/registry.json")).unwrap();
    let log = directory.path().join("race.log");
    register(&mut registry, "race", &log);

    let mut codex = BridgeClient::start(&config, &runtime);
    let mut claude = BridgeClient::start(&config, &runtime);
    let mut pi = BridgeClient::start(&config, &runtime);
    let request = || {
        json!({
            "name": "find_tools",
            "arguments": {"server": "race"},
            "_meta": protocol::metadata()
        })
    };
    let (codex_result, claude_result, pi_result) = tokio::join!(
        codex.call("tools/call", request()),
        claude.call("tools/call", request()),
        pi.call("tools/call", request())
    );
    assert_eq!(codex_result["result"]["resultType"], "complete");
    assert_eq!(claude_result["result"]["resultType"], "complete");
    assert_eq!(pi_result["result"]["resultType"], "complete");
    assert_eq!(
        std::fs::read_to_string(log)
            .unwrap()
            .matches("server/discover")
            .count(),
        1
    );

    let pid = daemon_pid(&runtime).expect("auto-started daemon was not running");
    Command::new("kill")
        .arg("-INT")
        .arg(pid.to_string())
        .status()
        .await
        .unwrap();
    codex.close().await;
    claude.close().await;
    pi.close().await;
    assert!(!stale.exists());
}
