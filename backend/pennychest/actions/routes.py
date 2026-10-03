from typing import Any

from fastapi import APIRouter, Body, Depends, HTTPException
from sqlalchemy.orm import Session

from pennychest.actions.catalog import (
    ACTIONS,
    ALL_SCOPES,
    ActionError,
    describe_actions,
    run_action,
)
from pennychest.core.database import get_db

router = APIRouter(prefix="/api/actions", tags=["actions"])


@router.get("")
def list_actions():
    return {"actions": describe_actions(ALL_SCOPES)}


@router.post("/{name}")
def call_action(
    name: str,
    arguments: dict[str, Any] = Body(default_factory=dict),
    db: Session = Depends(get_db),
):
    if name not in ACTIONS:
        raise HTTPException(status_code=404, detail=f"There is no action called {name!r}.")
    try:
        # A signed-in session has full access to the user's data.
        return run_action(db, name, arguments, scopes=ALL_SCOPES)
    except ActionError as e:
        raise HTTPException(status_code=400, detail=str(e))
