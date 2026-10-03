"""Typed operations on the user's data, shared by the in-app chat and the MCP server.

Each action declares its inputs as a Pydantic model (which doubles as the JSON schema given
to AI models) and the scope a caller needs to run it. Results are plain JSON-ready dicts built
from database queries, so numbers never come from a model doing arithmetic.
"""

import inspect
from collections.abc import Callable, Iterable
from dataclasses import dataclass
from enum import StrEnum
from typing import Any

from pydantic import BaseModel, ConfigDict, ValidationError
from sqlalchemy.orm import Session

from pennychest.actions.models import ActionChange


class ActionError(ValueError):
    """A problem with an action call, worded so an AI model or person can fix the call."""


class Scope(StrEnum):
    """What a caller is allowed to do. Read is always granted; the others add to it."""

    READ = "read"
    ORGANISE = "organise"  # categories, rules and budgets
    TRANSACTIONS = "transactions"  # the ledger itself


SCOPE_LABELS = {
    Scope.READ: "Look up your data",
    Scope.ORGANISE: "Change categories, rules and budgets",
    Scope.TRANSACTIONS: "Change transactions",
}

ALL_SCOPES = frozenset(Scope)


class ScopeNotAllowedError(ActionError):
    """An action called by someone without the scope it needs."""


def _granted(scopes: Iterable[Scope | str]) -> frozenset[Scope]:
    return frozenset({Scope.READ, *(Scope(s) for s in scopes)})


class ActionInput(BaseModel):
    model_config = ConfigDict(extra="forbid")


@dataclass(frozen=True)
class Action:
    name: str
    description: str
    scope: Scope
    input: type[ActionInput]
    handler: Callable[[Session, Any], dict]

    def input_schema(self) -> dict:
        schema = self.input.model_json_schema()
        schema.pop("title", None)
        for prop in schema.get("properties", {}).values():
            prop.pop("title", None)
        return schema


ACTIONS: dict[str, Action] = {}


def action(description: str, scope: Scope = Scope.READ):
    """Register a function `(db, params) -> dict` as an action named after the function."""

    def register(fn):
        params = list(inspect.signature(fn).parameters.values())
        input_model = params[1].annotation
        ACTIONS[fn.__name__] = Action(
            name=fn.__name__,
            description=" ".join(description.split()),
            scope=scope,
            input=input_model,
            handler=fn,
        )
        return fn

    return register


def describe_actions(scopes: Iterable[Scope | str] = ()) -> list[dict]:
    """The actions a caller with these scopes may run (read-only by default)."""
    granted = _granted(scopes)
    return [
        {
            "name": a.name,
            "description": a.description,
            "scope": a.scope.value,
            "input_schema": a.input_schema(),
        }
        for a in ACTIONS.values()
        if a.scope in granted
    ]


def run_action(
    db: Session,
    name: str,
    arguments: dict | None,
    *,
    scopes: Iterable[Scope | str] = (),
    source: str = "api",
    conversation_id: int | None = None,
) -> dict:
    """Run an action if the caller's scopes allow it. Callers are read-only unless they pass
    the extra scopes they hold, so one that forgets to check its permissions can only fail
    safe.

    Actions that change data return a "_change" entry (a summary and how to undo it), which
    is recorded here and replaced in the result by the change's id and summary."""
    found = ACTIONS.get(name)
    if not found:
        raise ActionError(f"There is no action called {name!r}.")
    if found.scope not in _granted(scopes):
        raise ScopeNotAllowedError(
            f"{name} needs the {found.scope.value!r} permission "
            f"({SCOPE_LABELS[found.scope].lower()}), which this caller doesn't have."
        )
    try:
        params = found.input.model_validate(arguments or {})
    except ValidationError as e:
        problems = "; ".join(
            f"{'.'.join(str(p) for p in err['loc']) or 'input'}: {err['msg']}" for err in e.errors()
        )
        raise ActionError(f"Invalid input for {name}: {problems}")
    result = found.handler(db, params)
    change = result.pop("_change", None)
    if change:
        record = ActionChange(
            action=name,
            arguments=arguments or {},
            summary=change["summary"],
            undo=change["undo"],
            source=source,
            conversation_id=conversation_id,
        )
        db.add(record)
        db.commit()
        result["change"] = {"id": record.id, "summary": record.summary}
    return result
