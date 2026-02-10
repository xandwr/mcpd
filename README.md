# mcpd

A daemon that aggregates multiple MCP (Model Context Protocol) servers into one.

Register any MCP server once with mcpd, then point your MCP client at mcpd. Add or remove servers at any time — agents discover new tools in realtime.

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

mcpd exposes exactly **two tools** to the MCP client, regardless of how many backend servers are registered:

- **`list_tools`** — Queries all registered backends and returns their tools (names, descriptions, input schemas).
- **`use_tool`** — Invokes a backend tool by its fully-qualified name (`server__tool`) with the given arguments.

The agent naturally calls `list_tools` first (it's the only way to know what's available), then calls `use_tool` to invoke what it needs. You can register or unregister backends at any time — the agent just calls `list_tools` again to see the latest.

```
┌─────────────────┐
│   MCP Client    │
│ (Claude, etc.)  │
└────────┬────────┘
         │ sees: list_tools, use_tool
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

### Workflow

1. You register MCP servers with mcpd (stored in `~/.config/mcpd/registry.json`)
2. Your MCP client connects to mcpd and sees two tools: `list_tools` and `use_tool`
3. Agent calls `list_tools` to discover available backend tools
4. Agent calls `use_tool(tool_name="server__tool", arguments={...})` to invoke them
5. mcpd spawns backend servers on-demand and proxies the call

### Example

After registering a filesystem server:

```
Agent calls: list_tools()
Returns:
  [{"name": "filesystem__read_file", "description": "Read a file", "input_schema": {...}},
   {"name": "filesystem__write_file", "description": "Write a file", "input_schema": {...}}]

Agent calls: use_tool(tool_name="filesystem__read_file", arguments={"path": "/tmp/hello.txt"})
Returns: contents of the file
```

## Why mcpd?

- **Register once**: Add servers to mcpd, not to every client
- **Realtime discovery**: Register/unregister servers without restarting clients — agents see changes on the next `list_tools` call
- **Stable interface**: Clients always see exactly two tools, no matter how many backends exist
- **Namespace isolation**: Tools from different servers can't collide (`server__tool` format)
- **On-demand**: Backend servers only spawn when their tools are actually invoked

## License

MIT
