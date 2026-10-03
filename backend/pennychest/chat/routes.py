import json
from collections.abc import Iterator

from fastapi import APIRouter, Depends, HTTPException
from fastapi.responses import StreamingResponse
from pydantic import BaseModel, Field
from sqlalchemy import select
from sqlalchemy.orm import Session

from pennychest.ai.config import get_chat_scopes
from pennychest.chat.models import ChatConversation
from pennychest.chat.service import display_turns, run_turn, start_conversation
from pennychest.core.database import get_db

router = APIRouter(prefix="/api/chat", tags=["chat"])


class SendMessage(BaseModel):
    message: str = Field(min_length=1, max_length=4000)
    conversation_id: int | None = None


def _conversation(db: Session, conversation_id: int) -> ChatConversation:
    conversation = db.get(ChatConversation, conversation_id)
    if not conversation:
        raise HTTPException(status_code=404, detail="Conversation not found")
    return conversation


@router.get("/conversations")
def list_conversations(db: Session = Depends(get_db)):
    conversations = (
        db.execute(
            select(ChatConversation).order_by(
                ChatConversation.updated_at.desc(), ChatConversation.id.desc()
            )
        )
        .scalars()
        .all()
    )
    return [
        {"id": c.id, "title": c.title, "updated_at": c.updated_at.isoformat()}
        for c in conversations
    ]


@router.get("/conversations/{conversation_id}")
def get_conversation(conversation_id: int, db: Session = Depends(get_db)):
    conversation = _conversation(db, conversation_id)
    return {
        "id": conversation.id,
        "title": conversation.title,
        "turns": display_turns(db, conversation),
    }


@router.delete("/conversations/{conversation_id}", status_code=204)
def delete_conversation(conversation_id: int, db: Session = Depends(get_db)):
    db.delete(_conversation(db, conversation_id))
    db.commit()


def _sse(event: str, data: dict) -> str:
    return f"event: {event}\ndata: {json.dumps(data, default=str)}\n\n"


@router.post("/messages")
def send_message(body: SendMessage, db: Session = Depends(get_db)):
    """Send a message and stream progress back as server-sent events: `conversation`, then
    any `tool` / `tool_result` steps, then `message` (or `error`), then `done`."""
    text = body.message.strip()
    if not text:
        raise HTTPException(status_code=400, detail="Message is empty")
    conversation = (
        _conversation(db, body.conversation_id)
        if body.conversation_id is not None
        else start_conversation(db, text)
    )

    def events() -> Iterator[str]:
        yield _sse("conversation", {"id": conversation.id, "title": conversation.title})
        for step in run_turn(db, conversation, text, scopes=get_chat_scopes(db)):
            yield _sse(step.pop("event"), step)
        yield _sse("done", {})

    return StreamingResponse(
        events(),
        media_type="text/event-stream",
        headers={"Cache-Control": "no-cache", "X-Accel-Buffering": "no"},
    )
