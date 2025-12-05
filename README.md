# mcpd

> An MCP daemon for automated tool registration.

- **Persistent daemon** - Runs at login, listens on a socket
- **Tool registration** - Each tool calls `mcpd register` on install (e.g., via post-install hook)
- **One MCP config** - User adds mcpd once, it proxies all registered tools
- **Auto-start** - systemd user service, launchd plist, or Windows service

## Architecture

```
Claude/Agent → mcpd serve (single MCP server)
                    ↓
              tool registry (~/.config/mcpd/registry.json)
                    ↓
         ┌─────────┼─────────┐
         ↓         ↓         ↓
       tool-a    tool-b    tool-c
```

## Installation

```bash
pip install mcpd
```

## Quick Start

### 1. Add mcpd to your Claude config

Add this to your Claude Desktop or Claude Code MCP configuration:

```json
{
    "mcpServers": {
        "mcpd": {
            "command": "mcpd",
            "args": ["serve"]
        }
    }
}
```

### 2. Register tools

```bash
# Register a tool by name and command
mcpd register mytool npx @my/mcp-server

# With environment variables
mcpd register mytool mytool --mcp -e API_KEY=xxx

# List registered tools
mcpd list

# Remove a tool
mcpd unregister mytool
```

### 3. (Optional) Start the daemon for hot-reloading

```bash
mcpd start
```

The daemon allows tools to be added/removed without restarting Claude.

## Commands

| Command | Description |
|---------|-------------|
| `mcpd register <name> <command...>` | Register an MCP tool |
| `mcpd unregister <name>` | Remove a registered tool |
| `mcpd list` | List all registered tools |
| `mcpd serve` | Run as MCP server (for Claude) |
| `mcpd start` | Start the background daemon |
| `mcpd status` | Check daemon status |
| `mcpd config` | Show configuration paths |

## The Install Experience

For tool authors, the ideal experience is:

```bash
cargo install mytool
# post-install hook runs: `mcpd register mytool`
# mytool is now available in Claude via MCP
```

### Adding post-install hooks

**Cargo (build.rs):**
```rust
// Can't run post-install :(
```

**npm (package.json):**
```json
{
    "scripts": {
        "postinstall": "mcpd register mypackage npx mypackage-mcp || true"
    }
}
```

**pip (pyproject.toml with hatch):**
```toml
[tool.hatch.build.hooks.custom]
# Custom hook to run mcpd register
```

## Auto-start (systemd)

```bash
# Copy service file
cp services/mcpd.service ~/.config/systemd/user/

# Enable and start
systemctl --user enable mcpd
systemctl --user start mcpd
```

## Auto-start (launchd / macOS)

```bash
cp services/com.mcpd.daemon.plist ~/Library/LaunchAgents/
launchctl load ~/Library/LaunchAgents/com.mcpd.daemon.plist
```

## Configuration

All configuration lives in `~/.config/mcpd/`:

- `registry.json` - Registered tools
- `server.log` - MCP server logs

## How It Works

1. **Registration**: `mcpd register` adds tool metadata to `~/.config/mcpd/registry.json`, i.e, `uv run mcpd register mytool mytool --mcp-manifest`
2. **Serving**: `mcpd serve` runs as an MCP server that Claude connects to
3. **Proxying**: When Claude requests tools, mcpd spawns the registered tool subprocesses and proxies MCP requests to them
4. **Aggregation**: Tools from all registered servers are combined (prefixed with `toolname__` to avoid collisions)

## License

MIT
