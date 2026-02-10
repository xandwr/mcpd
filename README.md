# mcpd

A daemon that aggregates multiple MCP (Model Context Protocol) servers into one. Implements the [MCP spec](https://spec.modelcontextprotocol.io/) (2025-11-25).

Register any MCP server once with mcpd, then point your MCP client at mcpd. Add or remove servers at any time — agents discover new tools, resources, and prompts in realtime.

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

mcpd aggregates all three MCP primitives — **tools**, **resources**, and **prompts** — from every registered backend into a single server.

### Tools (dual-layer)

mcpd exposes exactly **two meta-tools** to the MCP client, regardless of how many backend servers are registered:

- **`list_tools`** — Queries all registered backends and returns their tools (names, descriptions, input schemas).
- **`use_tool`** — Invokes a backend tool by its fully-qualified name (`server__tool`) with the given arguments.

The agent naturally calls `list_tools` first (it's the only way to know what's available), then calls `use_tool` to invoke what it needs. You can register or unregister backends at any time — the agent just calls `list_tools` again to see the latest.

### Resources

mcpd natively proxies `resources/list` and `resources/read`. Resources from all backends are aggregated and namespaced:

- URIs are prefixed: `mcpd://servername/original-uri`
- Names are prefixed: `servername__resourcename`

Backends that don't support resources are silently skipped.

### Prompts

mcpd natively proxies `prompts/list` and `prompts/get`. Prompts from all backends are aggregated and namespaced:

- Names are prefixed: `servername__promptname`

Backends that don't support prompts are silently skipped.

```
┌─────────────────┐
│   MCP Client    │
│ (Claude, etc.)  │
└────────┬────────┘
         │ tools, resources, prompts
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
2. Your MCP client connects to mcpd and sees two meta-tools (`list_tools`, `use_tool`) plus aggregated resources and prompts
3. Agent calls `list_tools` to discover available backend tools
4. Agent calls `use_tool(tool_name="server__tool", arguments={...})` to invoke them
5. Client can also call `resources/list`, `resources/read`, `prompts/list`, `prompts/get` directly
6. mcpd spawns backend servers on-demand and proxies the call

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

Resources and prompts work via standard MCP methods:

```
Client calls: resources/list
Returns:
  [{"uri": "mcpd://myserver/file:///docs/readme.md", "name": "myserver__readme", ...}]

Client calls: resources/read(uri="mcpd://myserver/file:///docs/readme.md")
Returns: resource contents

Client calls: prompts/list
Returns:
  [{"name": "myserver__summarize", "description": "Summarize a document", ...}]

Client calls: prompts/get(name="myserver__summarize", arguments={"text": "..."})
Returns: prompt messages
```

## Why mcpd?

- **Register once**: Add servers to mcpd, not to every client
- **Full MCP proxy**: Aggregates tools, resources, and prompts from all backends
- **Hot reload**: Register/unregister servers while mcpd is running — the registry is re-read on every request, and clients are notified via `list_changed` notifications
- **Stable interface**: Clients always see exactly two meta-tools, no matter how many backends exist
- **Namespace isolation**: Tools, resources, and prompts from different servers can't collide (`server__name` format)
- **On-demand**: Backend servers only spawn when actually needed
- **Graceful degradation**: Backends that don't support resources or prompts are silently skipped

## License

MIT
