# Agent integrations

## Setup

Configure a supported client with `mcpd setup <client>`. The built-in client integrations are `pi`, `codex`, and `claude`.

Codex and Claude use their native MCP configuration commands and connect to `mcpd serve` over stdio. Their setup applies across projects. Pi uses the bundled bridge described below.

## Pi

Install mcpd and its bundled extension:

```sh
cargo install mcpd
mcpd setup pi
```

Use `/reload` in Pi or start a new session. The extension exposes `mcpd_find_tools`, `mcpd_list_tools`, and `mcpd_use_tool`. Search by task keywords with `mcpd_find_tools`, then invoke a returned name using `mcpd_use_tool`. Use `mcpd_list_tools` when you need the complete catalog.

Setup copies the extension embedded in the installed binary into `~/.pi/agent/extensions/mcpd.ts`. It respects `PI_CODING_AGENT_DIR`. Re-run setup after upgrading mcpd; it replaces the previous copy or symlink. The extension runs `mcpd serve` from PATH on first use and closes the connection at session shutdown.

Requests time out after 120 seconds. Cancellation stops the affected request while other calls keep running. The bridge exposes tools only. Resources and prompts remain available through native MCP clients.

The extension uses Node built-ins and Pi's extension API, with no additional runtime packages.

## Native MCP clients

Configure a stdio server with command `mcpd` and arguments `["serve"]`. Discover backend tools with `find_tools`, then invoke their fully qualified names with `use_tool`. Restart the client connection after upgrading the binary.

## Protocol compatibility

mcpd supports [MCP 2026-07-28](https://modelcontextprotocol.io/specification/2026-07-28) over stdio, with a 2025-11-25 handshake fallback for legacy clients and backends. Modern requests carry version and capabilities in `_meta`; `server/discover` advertises both versions. Cacheable results use `ttlMs: 0` and `cacheScope: "private"` to keep discovery fresh.

Native clients can subscribe to registry list changes with `subscriptions/listen`. Changes are detected on the next backend discovery or routing request. Individual resource-update subscriptions are not supported and are omitted from the acknowledgment. Cancellation uses `notifications/cancelled`.

Tool calls, resource reads, and prompt requests preserve multi round-trip results and forward `inputResponses` and opaque `requestState` on retries. Rich content, structured results, and backend error data are preserved. Backend catalogs are paginated and aggregated in deterministic order.

Pi uses modern requests and retries state-only interim results up to three times. It does not advertise elicitation, roots, sampling, or optional extensions. HTTP transport and optional MCP extensions remain outside mcpd's supported interface.

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
