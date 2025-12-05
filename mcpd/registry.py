"""Tool registry for mcpd - stores registered MCP tools."""

import json
from dataclasses import dataclass, field, asdict
from pathlib import Path


def get_config_dir() -> Path:
    """Get the mcpd config directory."""
    config_dir = Path.home() / ".config" / "mcpd"
    config_dir.mkdir(parents=True, exist_ok=True)
    return config_dir


def get_registry_path() -> Path:
    """Get the path to the tool registry file."""
    return get_config_dir() / "registry.json"


def get_socket_path() -> Path:
    """Get the path to the daemon socket."""
    runtime_dir = Path(f"/run/user/{Path.home().stat().st_uid}")
    if runtime_dir.exists():
        return runtime_dir / "mcpd.sock"
    return get_config_dir() / "mcpd.sock"


@dataclass
class Tool:
    """A registered MCP tool."""

    name: str
    command: list[str]
    env: dict[str, str] = field(default_factory=dict)

    def to_dict(self) -> dict:
        return asdict(self)

    @classmethod
    def from_dict(cls, data: dict) -> "Tool":
        return cls(
            name=data["name"],
            command=data["command"],
            env=data.get("env", {}),
        )


class Registry:
    """Registry of MCP tools."""

    def __init__(self, path: Path | None = None):
        self.path = path or get_registry_path()
        self._tools: dict[str, Tool] = {}
        self._load()

    def _load(self) -> None:
        """Load the registry from disk."""
        if self.path.exists():
            data = json.loads(self.path.read_text())
            self._tools = {
                name: Tool.from_dict(tool) for name, tool in data.get("tools", {}).items()
            }

    def _save(self) -> None:
        """Save the registry to disk."""
        data = {"tools": {name: tool.to_dict() for name, tool in self._tools.items()}}
        self.path.write_text(json.dumps(data, indent=2) + "\n")

    def register(self, tool: Tool) -> None:
        """Register a tool."""
        self._tools[tool.name] = tool
        self._save()

    def unregister(self, name: str) -> bool:
        """Unregister a tool. Returns True if it existed."""
        if name in self._tools:
            del self._tools[name]
            self._save()
            return True
        return False

    def get(self, name: str) -> Tool | None:
        """Get a tool by name."""
        return self._tools.get(name)

    def list_tools(self) -> list[Tool]:
        """List all registered tools."""
        return list(self._tools.values())

    def reload(self) -> None:
        """Reload the registry from disk."""
        self._load()
