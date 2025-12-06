# mcpd

A daemon that aggregates multiple MCP (Model Context Protocol) servers into one.

Register any MCP server once with mcpd, then point your MCP client at mcpd. Add or remove servers without reconfiguring your client.

## Installation

```bash
cargo install mcpd
```

or for local:

```bash
cargo install --path .
```

## Usage

### Register a server

```bash
mcpd register <name> <command> [args...]
```

Examples:

```bash
# Register a Node.js MCP server
mcpd register filesystem npx -y @anthropic/mcp-filesystem /home/user/documents

# Register a Python server
mcpd register mytools python -m my_mcp_server

# Register with environment variables
mcpd register api-tools node server.js -e API_KEY=sk-xxx -e DEBUG=1
```

### List registered servers

```bash
mcpd list
```

### Remove a server

```bash
mcpd unregister <name>
```

### Run the daemon

```bash
mcpd serve
```

This starts mcpd in stdio mode, ready to accept MCP connections.

## Client Configuration

Point your MCP client at mcpd instead of individual servers.

**Claude Code** (`~/.claude/settings.json`):

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

**Claude Desktop** (`claude_desktop_config.json`):

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

**VSCode** (Ctrl+Shift+P, search 'MCP', click 'MCP: Add Server', select 'stdio', type `mcpd serve`)

## How It Works

1. You register MCP servers with mcpd (stored in `~/.config/mcpd/registry.json`)
2. Your MCP client connects to mcpd
3. mcpd spawns registered servers on-demand and proxies requests to them
4. Tools are namespaced as `<server>__<tool>` to avoid collisions

```
┌─────────────────┐
│   MCP Client    │
│ (Claude, etc.)  │
└────────┬────────┘
         │ stdio
         ▼
┌─────────────────┐
│      mcpd       │
└──┬─────┬─────┬──┘
   │     │     │ stdio (spawned on-demand)
   ▼     ▼     ▼
┌─────┐┌─────┐┌─────┐
│ srv1││ srv2││ srv3│
└─────┘└─────┘└─────┘
```

## Why mcpd?

- **Single config**: Add servers to mcpd, not to every client
- **Hot-swap**: Register/unregister servers without restarting clients
- **Namespace isolation**: Tools from different servers can't collide
- **On-demand**: Servers only spawn when their tools are invoked

## License

MIT
