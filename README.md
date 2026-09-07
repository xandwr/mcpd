# mcpd

Register MCP servers once. Give your agents one connection.

mcpd routes tools, resources, and prompts from registered stdio servers through a single MCP endpoint. Backends start on demand; registration changes take effect on the next discovery call. Supports MCP 2026-07-28 and legacy 2025-11-25 clients and servers.

## Install and connect

```sh
cargo install mcpd --locked
mcpd register <name> <command> [args...]
```

Add `-e KEY=VALUE` to a registration to set backend environment variables. Use `mcpd list` to inspect registrations and `mcpd unregister <name>` to remove one. The registry lives in your OS config directory under `mcpd/registry.json`.

Configure your MCP client to launch **`mcpd serve`** over stdio:

```json
{
  "mcpServers": {
    "mcpd": { "command": "mcpd", "args": ["serve"] }
  }
}
```

For **Pi**, run `mcpd setup pi`, then `/reload`. Repeat after upgrading. The bundled extension runs mcpd from PATH and prefixes its tools with `mcpd_`.

## Discover and use

Agents see three tools regardless of backend count:

| Tool | Purpose |
| --- | --- |
| `find_tools` | Search server names, tool names, and descriptions; return ranked matches with input schemas. |
| `list_tools` | Return the complete backend tool catalog. |
| `use_tool` | Call a discovered `server__tool` with `tool_name` and `arguments`. |

Start with `find_tools({"query":"file search"})`. Search matches any keyword, ignoring case, and favors names over descriptions. Omit `query` to browse; use `server` for an exact server filter. `limit` defaults to 10 (maximum 100). Results include `total_matches`, registered `servers`, and backend `errors`; each backend gets 15 seconds to respond.

Resources and prompts use standard MCP methods. Names become `server__name`; resource URIs become `mcpd://server/original-uri`. Backends lacking either capability are skipped.

[Integration and development details](integrations/README.md) | [MIT license](LICENSE)
