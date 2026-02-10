# CLAUDE.md

## What is mcpd?

mcpd is a daemon that aggregates multiple MCP (Model Context Protocol) servers into a single endpoint. Register MCP servers once, point any client at mcpd, add/remove servers at runtime. Written in Rust, MIT licensed.

**Repository:** https://github.com/xandwr/mcpd
**Crate:** https://crates.io/crates/mcpd
**MCP spec version:** 2025-11-25

## Architecture

```
Client (stdio) → Server → [Registry] → ToolProxy (per backend) → subprocess (stdio)
```

Six source files in `src/`:

- **main.rs** — Entry point. Initializes tracing (stderr, `RUST_LOG`), parses CLI, runs command.
- **cli.rs** — clap-based CLI. Four subcommands: `register`, `unregister`, `list`, `serve`. Resolves command paths via `which`.
- **server.rs** — The aggregating MCP server. Listens on stdin/stdout. Exposes two meta-tools (`list_tools`, `use_tool`) and natively proxies resources and prompts. Syncs registry from disk on every request and sends `list_changed` notifications on changes.
- **proxy.rs** — `ToolProxy` manages one backend subprocess. Handles spawn, MCP initialization handshake, JSON-RPC request/response matching via oneshot channels, and clean shutdown. On-demand — only starts when needed.
- **registry.rs** — Persistent JSON storage at `~/.config/mcpd/registry.json`. Stores tool name, command (resolved path + args), and per-server environment variables. Supports reload from disk.
- **mcp.rs** — All MCP/JSON-RPC protocol types. Request, Response, Notification, plus MCP-specific types for tools, resources, prompts. No logic, just serialization.

## Key design decisions

- **Dual-layer tool system:** mcpd exposes exactly 2 tools to clients regardless of backend count. Agents call `list_tools` to discover, `use_tool` to invoke. This keeps the client interface stable.
- **Namespace isolation:** All names use `server__name` format (double underscore). Resource URIs use `mcpd://server/original-uri`.
- **Filesystem as coordination:** Registry is re-read from disk on every request. No file watchers, no IPC. `mcpd register` writes JSON, `mcpd serve` reads it. Simple.
- **Graceful degradation:** Backends that don't support resources or prompts are silently skipped (logged at debug level).
- **No async read loop:** Proxy reads stdout synchronously in `read_until_response` while holding the lock. Works because each proxy handles one request at a time.

## Building and running

```bash
cargo build            # dev build
cargo install --path . # install locally
cargo install mcpd     # install from crates.io
```

No tests currently. No CI. No config files beyond Cargo.toml.

## Dependencies

tokio (async runtime), serde/serde_json (serialization), clap (CLI), anyhow/thiserror (errors), tracing/tracing-subscriber (logging to stderr), dirs (config dir), which (PATH resolution).

## Conventions

- Rust 2024 edition
- No `unsafe`, no proc macros beyond derive
- Logging goes to stderr (stdout is the MCP transport)
- Error handling: `anyhow::Result` everywhere, `thiserror` available but not currently used for custom error types
- Keep it minimal — the whole codebase is ~1200 lines and that's a feature
- Commit messages are short and informal
