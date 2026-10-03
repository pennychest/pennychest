"""Importing this module registers every action."""

from pennychest.actions import insights, ledger, read, trends, write  # noqa: F401
from pennychest.actions.registry import (
    ACTIONS,
    ALL_SCOPES,
    SCOPE_LABELS,
    ActionError,
    Scope,
    ScopeNotAllowedError,
    describe_actions,
    run_action,
)

__all__ = [
    "ACTIONS",
    "ALL_SCOPES",
    "SCOPE_LABELS",
    "ActionError",
    "Scope",
    "ScopeNotAllowedError",
    "describe_actions",
    "run_action",
]
