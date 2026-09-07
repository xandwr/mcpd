# Local agent integrations

## Codex

Register the aggregator once:

```sh
codex mcp add mcpd -- "$HOME/.cargo/bin/mcpd" serve
```

Start a new Codex session after changing its MCP configuration. Discover backend tools with `list_tools`, then invoke their fully qualified names with `use_tool`.

## Pi

Link the extension into Pi's global extension directory:

```sh
mkdir -p "$HOME/.pi/agent/extensions"
ln -s /home/xander/Projects/mcpd/integrations/pi.ts "$HOME/.pi/agent/extensions/mcpd.ts"
```

Use `/reload` in Pi or start a new session. The extension exposes `mcpd_list_tools` and `mcpd_use_tool`. It starts `$HOME/.cargo/bin/mcpd serve` on first use and closes the connection at session shutdown. Requests time out after 120 seconds. Cancellation closes the shared connection and rejects any pending calls; the next call reconnects. The bridge exposes tools only; resources and prompts remain available through native MCP clients such as Codex.

The extension uses Node built-ins and Pi's extension API, with no additional runtime packages. Its source stays in this repository through the symlink.

## Godot setup on this machine

```sh
cargo build --locked --release --manifest-path /home/xander/Projects/reflection-engine/tools/godot-mcp/Cargo.toml
mcpd register godot /home/xander/Projects/reflection-engine/tools/godot-mcp/target/release/godot-mcp -e GODOT_BIN=/usr/bin/godot
```

Rebuild after editing the Godot server. Restart connected agent sessions to replace an already running backend process.

Call `mcpd_list_tools` in Pi, then `mcpd_use_tool` with:

```json
{
  "tool_name": "godot__godot_scene_inspect",
  "arguments": {
    "project_root": "/home/xander/Projects/reflection-engine",
    "scene_path": "res://scenes/main.tscn"
  }
}
```

Use the equivalent `use_tool` in Codex. Close the project's GUI editor before scene inspection. Registration is shared through `~/.config/mcpd/registry.json`; project paths are supplied per tool call.
