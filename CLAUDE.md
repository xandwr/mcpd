# CLAUDE.md

## What is mcpd?

mcpd is a daemon that aggregates multiple MCP (Model Context Protocol) servers into a single endpoint. Register MCP servers once, point any client at mcpd, add/remove servers at runtime. Written in Rust, MIT licensed.

**Repository:** https://github.com/xandwr/mcpd
**Crate:** https://crates.io/crates/mcpd
**MCP spec versions:** 2026-07-28 and legacy 2025-11-25

## Architecture

```
Client -> Server -> Hub -> [Registry] -> BackendSession (per backend) -> subprocess (stdio)
```

Source files in `src/`:

- **main.rs** - Entry point. Initializes tracing (stderr, `RUST_LOG`), parses CLI, runs command.
- **cli.rs** - clap-based CLI. Subcommands: `register`, `unregister`, `list`, `serve`, `daemon`, `setup pi`. Setup installs the bundled Pi extension. Resolves command paths via `which`.
- **daemon.rs** - The Unix socket listener at `$XDG_RUNTIME_DIR/mcpd.sock`. Accepts concurrent client connections backed by one shared hub and shuts down cleanly on SIGINT or SIGTERM.
- **discovery.rs** - Search parameters, ranked tool results, and keyword matching without extra dependencies.
- **server.rs** - The transport-neutral client connection and shared aggregation hub. `Server` owns client initialization, subscriptions, cancellation, and output. `Hub` owns the registry, change broadcasts, and reusable backend sessions. Exposes three meta-tools (`find_tools`, `list_tools`, `use_tool`) and natively proxies resources and prompts.
- **proxy.rs** - `ToolProxy` manages one backend subprocess. Handles spawn, modern discovery with legacy handshake fallback, queued writes, JSON-RPC response matching, pagination, cancellation, and shutdown. On-demand - only starts when needed.
- **registry.rs** - Persistent JSON storage at `~/.config/mcpd/registry.json`. Stores tool name, command (resolved path + args), and per-server environment variables. Supports reload from disk.
- **protocol.rs** - Modern request validation, version errors, server identity, and result/cache metadata.
- **mcp.rs** - All MCP/JSON-RPC protocol types. Request, Response, Notification, plus MCP-specific types for tools, resources, prompts. No logic, just serialization.

## Key design decisions

- **Dual-layer tool system:** mcpd exposes exactly 3 tools to clients regardless of backend count. Agents call `find_tools` to search or `list_tools` to enumerate, `use_tool` to invoke. This keeps the client interface stable.
- **Namespace isolation:** All names use `server__name` format (double underscore). Resource URIs use `mcpd://server/original-uri`.
- **Filesystem as coordination:** Registry is re-read from disk on every request. No file watchers, no IPC. `mcpd register` writes JSON, `mcpd serve` reads it. Simple.
- **Graceful degradation:** Backends that don't support resources or prompts are silently skipped (logged at debug level).
- **Background response reader:** Each proxy dispatches responses to pending requests using oneshot channels. Protocol detection is serialized separately. The server handles requests concurrently and cancels them by request ID.
- **Shared daemon isolation:** Unix socket clients share backend sessions through one hub while request IDs, subscriptions, cancellation, and output remain scoped to each connection.

## Building and running

```bash
cargo build            # dev build
cargo install --path . # install locally
cargo install mcpd     # install from crates.io
```

## Testing

```bash
cargo test                  # unit tests only
cargo test --features _test # all tests (unit + integration with mock MCP server)
cargo clippy --all-targets --features _test -- -D warnings  # lint
cargo fmt -- --check        # format check
```

Tests are organized as:
- Inline `#[cfg(test)]` modules in `mcp.rs`, `registry.rs`, `cli.rs` for unit tests
- `tests/integration.rs` for proxy integration tests using a mock MCP server
- `test-support/mock_mcp_server.rs` is a minimal MCP server binary for testing (gated behind `_test` feature)

## CI/CD

CI runs on every push/PR to `main` (`.github/workflows/ci.yml`): check, clippy, fmt, test.

Releases are triggered by pushing `v*` tags (`.github/workflows/release.yml`): verifies version match, runs tests, publishes to crates.io, creates GitHub Release. Requires `CARGO_REGISTRY_TOKEN` secret.

## Dependencies

tokio (async runtime), serde/serde_json (serialization), clap (CLI), anyhow/thiserror (errors), tracing/tracing-subscriber (logging to stderr), dirs (config dir), which (PATH resolution).

## Conventions

- Rust 2024 edition
- No `unsafe`, no proc macros beyond derive
- Logging goes to stderr (stdout is the MCP transport)
- Error handling: `anyhow::Result` everywhere, `thiserror` available but not currently used for custom error types
- Keep it minimal - the whole codebase is ~1200 lines and that's a feature
- Commit messages are short and informal
