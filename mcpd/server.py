"""MCP server that aggregates all registered tools."""

import asyncio
import json
import logging
import os
from contextlib import asynccontextmanager
from typing import Any

from mcp.server import Server
from mcp.server.stdio import stdio_server
from mcp.types import Tool as MCPTool, TextContent

from .registry import Registry, Tool

log = logging.getLogger(__name__)


class ToolProxy:
    """Proxy for communicating with a tool subprocess."""

    def __init__(self, tool: Tool):
        self.tool = tool
        self.process: asyncio.subprocess.Process | None = None
        self._request_id = 0
        self._lock = asyncio.Lock()
        self._pending: dict[int, asyncio.Future] = {}
        self._reader_task: asyncio.Task | None = None

    async def start(self) -> None:
        """Start the tool subprocess."""
        async with self._lock:
            if self.process is not None and self.process.returncode is None:
                return

            env = os.environ.copy()
            env.update(self.tool.env)

            self.process = await asyncio.create_subprocess_exec(
                *self.tool.command,
                stdin=asyncio.subprocess.PIPE,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
                env=env,
            )
            self._reader_task = asyncio.create_task(self._read_responses())
            log.info(f"Started tool proxy for {self.tool.name} (pid={self.process.pid})")

    async def stop(self) -> None:
        """Stop the tool subprocess."""
        async with self._lock:
            if self._reader_task:
                self._reader_task.cancel()
                self._reader_task = None

            if self.process is None:
                return

            self.process.terminate()
            try:
                await asyncio.wait_for(self.process.wait(), timeout=5.0)
            except asyncio.TimeoutError:
                self.process.kill()
                await self.process.wait()

            # Cancel pending requests
            for future in self._pending.values():
                future.cancel()
            self._pending.clear()

            log.info(f"Stopped tool proxy for {self.tool.name}")
            self.process = None

    async def _read_responses(self) -> None:
        """Read responses from the subprocess."""
        if not self.process or not self.process.stdout:
            return

        try:
            while True:
                line = await self.process.stdout.readline()
                if not line:
                    break

                try:
                    response = json.loads(line.decode())
                    request_id = response.get("id")
                    if request_id in self._pending:
                        self._pending[request_id].set_result(response)
                except json.JSONDecodeError:
                    log.warning(f"Invalid JSON from {self.tool.name}: {line}")
        except asyncio.CancelledError:
            pass
        except Exception as e:
            log.exception(f"Error reading from {self.tool.name}: {e}")

    async def call(self, method: str, params: dict | None = None) -> Any:
        """Make a JSON-RPC call to the tool."""
        if self.process is None or self.process.returncode is not None:
            await self.start()

        if not self.process or not self.process.stdin:
            raise RuntimeError(f"Tool {self.tool.name} not running")

        self._request_id += 1
        request_id = self._request_id

        request = {
            "jsonrpc": "2.0",
            "id": request_id,
            "method": method,
            "params": params or {},
        }

        future: asyncio.Future = asyncio.get_event_loop().create_future()
        self._pending[request_id] = future

        try:
            request_line = json.dumps(request) + "\n"
            self.process.stdin.write(request_line.encode())
            await self.process.stdin.drain()

            response = await asyncio.wait_for(future, timeout=30.0)
            if "error" in response:
                raise RuntimeError(response["error"].get("message", "Unknown error"))
            return response.get("result")
        finally:
            self._pending.pop(request_id, None)


class AggregatedServer:
    """MCP server that aggregates tools from multiple MCP tool servers."""

    def __init__(self, registry: Registry | None = None):
        self.registry = registry or Registry()
        self.proxies: dict[str, ToolProxy] = {}
        self.tool_map: dict[str, str] = {}  # tool_name -> proxy_name
        self.server = Server("mcpd")
        self._setup_handlers()

    def _setup_handlers(self) -> None:
        """Set up MCP server handlers."""

        @self.server.list_tools()
        async def list_tools() -> list[MCPTool]:
            await self._ensure_proxies()
            tools = []
            for proxy_name, proxy in self.proxies.items():
                try:
                    result = await proxy.call("tools/list")
                    for tool in result.get("tools", []):
                        # Prefix tool name with proxy name to avoid collisions
                        prefixed_name = f"{proxy_name}__{tool['name']}"
                        self.tool_map[prefixed_name] = proxy_name
                        tools.append(
                            MCPTool(
                                name=prefixed_name,
                                description=tool.get("description", ""),
                                inputSchema=tool.get("inputSchema", {"type": "object"}),
                            )
                        )
                except Exception as e:
                    log.warning(f"Failed to list tools from {proxy_name}: {e}")
            return tools

        @self.server.call_tool()
        async def call_tool(name: str, arguments: dict) -> list[TextContent]:
            await self._ensure_proxies()

            proxy_name = self.tool_map.get(name)
            if not proxy_name:
                return [TextContent(type="text", text=f"Unknown tool: {name}")]

            proxy = self.proxies.get(proxy_name)
            if not proxy:
                return [TextContent(type="text", text=f"Tool proxy not found: {proxy_name}")]

            # Strip the prefix to get the original tool name
            original_name = name.split("__", 1)[1] if "__" in name else name

            try:
                result = await proxy.call("tools/call", {"name": original_name, "arguments": arguments})
                content = result.get("content", [])
                return [
                    TextContent(type="text", text=c.get("text", str(c)))
                    for c in content
                ]
            except Exception as e:
                log.exception(f"Error calling tool {name}")
                return [TextContent(type="text", text=f"Error: {e}")]

    async def _ensure_proxies(self) -> None:
        """Ensure all tools have proxies."""
        self.registry.reload()
        for tool in self.registry.list_tools():
            if tool.name not in self.proxies:
                self.proxies[tool.name] = ToolProxy(tool)

    async def run(self) -> None:
        """Run the aggregated MCP server."""
        async with stdio_server() as (read_stream, write_stream):
            await self.server.run(read_stream, write_stream, self.server.create_initialization_options())

    async def stop(self) -> None:
        """Stop all proxies."""
        for proxy in self.proxies.values():
            await proxy.stop()


async def run_server() -> None:
    """Run the mcpd MCP server."""
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s - %(name)s - %(levelname)s - %(message)s",
        handlers=[logging.FileHandler(os.path.expanduser("~/.config/mcpd/server.log"))],
    )

    server = AggregatedServer()
    try:
        await server.run()
    finally:
        await server.stop()
