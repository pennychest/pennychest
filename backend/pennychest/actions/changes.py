from fastapi import APIRouter, Depends, HTTPException
from sqlalchemy import func, select
from sqlalchemy.orm import Session

from pennychest.actions.models import ActionChange
from pennychest.actions.undo import UndoError, undo_change
from pennychest.core.database import get_db

router = APIRouter(prefix="/api/changes", tags=["changes"])


def serialise(change: ActionChange) -> dict:
    return {
        "id": change.id,
        "created_at": change.created_at.isoformat(),
        "action": change.action,
        "summary": change.summary,
        "source": change.source,
        "conversation_id": change.conversation_id,
        "undone_at": change.undone_at.isoformat() if change.undone_at else None,
    }


@router.get("")
def list_changes(limit: int = 50, offset: int = 0, db: Session = Depends(get_db)):
    limit = min(max(limit, 1), 200)
    total = db.execute(select(func.count()).select_from(ActionChange)).scalar_one()
    changes = (
        db.execute(
            select(ActionChange)
            .order_by(ActionChange.id.desc())
            .limit(limit)
            .offset(max(offset, 0))
        )
        .scalars()
        .all()
    )
    return {"total": total, "changes": [serialise(c) for c in changes]}


@router.post("/{change_id}/undo")
def undo(change_id: int, db: Session = Depends(get_db)):
    change = db.get(ActionChange, change_id)
    if not change:
        raise HTTPException(status_code=404, detail="Change not found")
    try:
        undo_change(db, change)
    except UndoError as e:
        raise HTTPException(status_code=400, detail=str(e))
    return serialise(change)
