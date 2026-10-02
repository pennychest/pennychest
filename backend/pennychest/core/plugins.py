"""Loading plugins registered as Python entry points.

Each plugin type has its own entry-point group (e.g. 'pennychest.importers',
'pennychest.exporters'). First-party plugins register in pyproject.toml exactly
as third-party packages would.
"""

from importlib.metadata import entry_points
from typing import Any


def load_plugins(group: str) -> dict[str, Any]:
    """An instance of each plugin registered in the group, by entry-point name.
    Plugins that fail to load are skipped."""
    plugins: dict[str, Any] = {}
    for ep in entry_points(group=group):
        try:
            plugins[ep.name] = ep.load()()
        except Exception:
            continue
    return plugins
