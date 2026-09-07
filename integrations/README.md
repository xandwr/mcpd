# Agent integrations

## Pi

Install mcpd and its bundled extension:

```sh
cargo install mcpd
mcpd setup pi
```

Use `/reload` in Pi or start a new session. The extension exposes `mcpd_find_tools`, `mcpd_list_tools`, and `mcpd_use_tool`. Search by task keywords with `mcpd_find_tools`, then invoke a returned name using `mcpd_use_tool`. Use `mcpd_list_tools` when you need the complete catalog.

Setup copies the extension embedded in the installed binary into `~/.pi/agent/extensions/mcpd.ts`. It respects `PI_CODING_AGENT_DIR`. Re-run setup after upgrading mcpd; it replaces the previous copy or symlink. The extension runs `mcpd serve` from PATH on first use and closes the connection at session shutdown.

Requests time out after 120 seconds. Cancellation closes the shared connection and rejects pending calls; the next call reconnects. The bridge exposes tools only. Resources and prompts remain available through native MCP clients.

The extension uses Node built-ins and Pi's extension API, with no additional runtime packages.

## Native MCP clients

Configure a stdio server with command `mcpd` and arguments `["serve"]`. Discover backend tools with `find_tools`, then invoke their fully qualified names with `use_tool`. Restart the client connection after upgrading the binary.

## Local development

Build and install from the verified Cargo package, then reload Pi:

```sh
cargo test --locked --features _test
cargo package --allow-dirty --locked
cargo install --path target/package/mcpd-1.0.7 --locked --force
mcpd setup pi
```

`--allow-dirty` includes uncommitted changes without publishing them. Use the installed extension and binary for normal work. Source changes take effect after the next packaged installation and setup, just as they do for a user upgrading a release.

To exercise the installed Pi bridge with an isolated registry and mock backend:

```sh
cargo build --locked --features _test --bin mock-mcp-server
node --experimental-strip-types test-support/pi-smoke.mjs
```

This checks the installed `mcpd` from PATH, installs its bundled extension into a temporary Pi directory, and calls discovery and invocation through that extension. It does not modify your registered servers.
