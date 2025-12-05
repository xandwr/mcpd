"""mcpd CLI - command-line interface for the MCP daemon."""

import asyncio
import json
import shutil
import sys

import click

from .registry import Registry, Tool, get_socket_path


def send_daemon_command(command: dict) -> dict | None:
    """Send a command to the daemon and get response."""
    socket_path = get_socket_path()
    if not socket_path.exists():
        return None

    async def _send():
        try:
            reader, writer = await asyncio.open_unix_connection(str(socket_path))
            writer.write(json.dumps(command).encode() + b"\n")
            await writer.drain()
            response = await reader.readline()
            writer.close()
            await writer.wait_closed()
            return json.loads(response.decode())
        except Exception:
            return None

    return asyncio.run(_send())


def notify_daemon() -> None:
    """Notify the daemon to reload its registry."""
    send_daemon_command({"cmd": "reload"})


@click.group()
@click.version_option()
def main():
    """mcpd - MCP daemon for centralized tool registration."""
    pass


@main.command(context_settings={"ignore_unknown_options": True, "allow_interspersed_args": False})
@click.argument("name")
@click.argument("command", nargs=-1, required=True, type=click.UNPROCESSED)
@click.option("--env", "-e", multiple=True, help="Environment variable (KEY=VALUE)")
def register(name: str, command: tuple[str, ...], env: tuple[str, ...]):
    """Register an MCP tool.

    NAME is the tool identifier.
    COMMAND is the command to run the tool (e.g., 'npx @tool/server').

    Example:
        mcpd register mytool npx @my/mcp-server
        mcpd register archmap archmap --mcp-manifest
        mcpd register -e API_KEY=xxx myapi myapi-server --port 8080
    """
    # Resolve command to full path if it's a bare executable
    resolved_command = list(command)
    exe = shutil.which(command[0])
    if exe:
        resolved_command[0] = exe

    env_dict = {}
    for e in env:
        if "=" in e:
            key, value = e.split("=", 1)
            env_dict[key] = value

    tool = Tool(name=name, command=resolved_command, env=env_dict)

    registry = Registry()
    registry.register(tool)

    click.echo(f"Registered tool: {name}")
    click.echo(f"  Command: {' '.join(resolved_command)}")
    if env_dict:
        click.echo(f"  Environment: {env_dict}")

    notify_daemon()


@main.command()
@click.argument("name")
def unregister(name: str):
    """Unregister an MCP tool."""
    registry = Registry()
    if registry.unregister(name):
        click.echo(f"Unregistered tool: {name}")
        notify_daemon()
    else:
        click.echo(f"Tool not found: {name}", err=True)
        sys.exit(1)


@main.command("list")
def list_tools():
    """List all registered tools."""
    registry = Registry()
    tools = registry.list_tools()

    if not tools:
        click.echo("No tools registered.")
        return

    for tool in tools:
        click.echo(f"{tool.name}")
        click.echo(f"  Command: {' '.join(tool.command)}")
        if tool.env:
            click.echo(f"  Environment: {tool.env}")


@main.command()
def start():
    """Start the mcpd daemon."""
    from .daemon import run_daemon

    click.echo("Starting mcpd daemon...")
    asyncio.run(run_daemon())


@main.command()
def serve():
    """Run as an MCP server (for Claude/agents to connect to).

    This is what you add to your Claude MCP config:
        {
            "mcpServers": {
                "mcpd": {
                    "command": "mcpd",
                    "args": ["serve"]
                }
            }
        }
    """
    from .server import run_server

    asyncio.run(run_server())


@main.command()
def status():
    """Check daemon status."""
    response = send_daemon_command({"cmd": "ping"})
    if response and response.get("status") == "ok":
        click.echo("Daemon is running")

        # Get tool count
        list_response = send_daemon_command({"cmd": "list"})
        if list_response:
            tools = list_response.get("tools", [])
            click.echo(f"Tools loaded: {len(tools)}")
    else:
        click.echo("Daemon is not running")
        sys.exit(1)


@main.command()
def config():
    """Show configuration paths."""
    from .registry import get_config_dir, get_registry_path, get_socket_path

    click.echo(f"Config directory: {get_config_dir()}")
    click.echo(f"Registry file:    {get_registry_path()}")
    click.echo(f"Socket path:      {get_socket_path()}")


if __name__ == "__main__":
    main()
