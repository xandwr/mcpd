"""mcpd daemon - manages tool subprocesses and aggregates MCP."""

import asyncio
import json
import logging
import os
import signal
import sys
from pathlib import Path

from .registry import Registry, Tool, get_socket_path

log = logging.getLogger(__name__)


class ToolProcess:
    """Manages a single MCP tool subprocess."""

    def __init__(self, tool: Tool):
        self.tool = tool
        self.process: asyncio.subprocess.Process | None = None
        self._lock = asyncio.Lock()

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
            log.info(f"Started tool {self.tool.name} (pid={self.process.pid})")

    async def stop(self) -> None:
        """Stop the tool subprocess."""
        async with self._lock:
            if self.process is None:
                return

            self.process.terminate()
            try:
                await asyncio.wait_for(self.process.wait(), timeout=5.0)
            except asyncio.TimeoutError:
                self.process.kill()
                await self.process.wait()
            log.info(f"Stopped tool {self.tool.name}")
            self.process = None

    async def send_request(self, request: dict) -> dict | None:
        """Send a JSON-RPC request to the tool and get response."""
        if self.process is None or self.process.returncode is not None:
            await self.start()

        if self.process is None or self.process.stdin is None or self.process.stdout is None:
            return None

        request_line = json.dumps(request) + "\n"
        self.process.stdin.write(request_line.encode())
        await self.process.stdin.drain()

        response_line = await self.process.stdout.readline()
        if not response_line:
            return None

        return json.loads(response_line.decode())


class Daemon:
    """The mcpd daemon - aggregates multiple MCP tools into one server."""

    def __init__(self, registry: Registry | None = None):
        self.registry = registry or Registry()
        self.tools: dict[str, ToolProcess] = {}
        self._server: asyncio.Server | None = None
        self._running = False

    async def start(self) -> None:
        """Start the daemon."""
        self._running = True
        self._load_tools()

        socket_path = get_socket_path()
        if socket_path.exists():
            socket_path.unlink()

        self._server = await asyncio.start_unix_server(
            self._handle_control_client,
            path=str(socket_path),
        )
        socket_path.chmod(0o600)

        log.info(f"Daemon listening on {socket_path}")

        # Handle signals
        loop = asyncio.get_event_loop()
        for sig in (signal.SIGTERM, signal.SIGINT):
            loop.add_signal_handler(sig, lambda: asyncio.create_task(self.stop()))

    def _load_tools(self) -> None:
        """Load tools from registry."""
        self.registry.reload()
        for tool in self.registry.list_tools():
            if tool.name not in self.tools:
                self.tools[tool.name] = ToolProcess(tool)

    async def stop(self) -> None:
        """Stop the daemon."""
        self._running = False

        for tool in self.tools.values():
            await tool.stop()

        if self._server:
            self._server.close()
            await self._server.wait_closed()

        socket_path = get_socket_path()
        if socket_path.exists():
            socket_path.unlink()

        log.info("Daemon stopped")

    async def _handle_control_client(
        self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter
    ) -> None:
        """Handle a control connection (for register/unregister commands)."""
        try:
            data = await reader.readline()
            if not data:
                return

            command = json.loads(data.decode())
            response = await self._handle_control_command(command)
            writer.write(json.dumps(response).encode() + b"\n")
            await writer.drain()
        except Exception as e:
            log.exception("Error handling control client")
            writer.write(json.dumps({"error": str(e)}).encode() + b"\n")
            await writer.drain()
        finally:
            writer.close()
            await writer.wait_closed()

    async def _handle_control_command(self, command: dict) -> dict:
        """Handle a control command."""
        cmd = command.get("cmd")

        if cmd == "reload":
            self._load_tools()
            return {"status": "ok", "tools": len(self.tools)}

        elif cmd == "list":
            return {
                "status": "ok",
                "tools": [
                    {"name": t.tool.name, "command": t.tool.command}
                    for t in self.tools.values()
                ],
            }

        elif cmd == "ping":
            return {"status": "ok"}

        return {"error": f"unknown command: {cmd}"}

    async def run_forever(self) -> None:
        """Run the daemon until stopped."""
        await self.start()
        while self._running:
            await asyncio.sleep(1)


async def run_daemon() -> None:
    """Run the mcpd daemon."""
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s - %(name)s - %(levelname)s - %(message)s",
    )

    daemon = Daemon()
    await daemon.run_forever()
