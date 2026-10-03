"""Personal access tokens for the MCP server, managed from Settings."""

import datetime as dt
import hashlib
import secrets
import time

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel
from sqlalchemy import select
from sqlalchemy.orm import Session

from pennychest.actions.registry import Scope
from pennychest.core.database import get_db
from pennychest.mcp.models import AccessToken

TOKEN_PREFIX = "pc_"
# The scopes a token can add on top of read, in the order they're shown
WRITE_SCOPES = (Scope.ORGANISE.value, Scope.TRANSACTIONS.value)

router = APIRouter(prefix="/api/mcp/tokens", tags=["mcp"])


def _hash(token: str) -> str:
    return hashlib.sha256(token.encode()).hexdigest()


def create_token(db: Session, name: str, scopes: list[str]) -> tuple[AccessToken, str]:
    raw = TOKEN_PREFIX + secrets.token_urlsafe(32)
    token = AccessToken(
        name=name,
        token_hash=_hash(raw),
        prefix=raw[:10],
        scopes=[s for s in WRITE_SCOPES if s in scopes],
    )
    db.add(token)
    db.commit()
    return token, raw


def find_token(db: Session, raw: str | None) -> AccessToken | None:
    """The token this bearer value belongs to, noting that it was used."""
    if not raw or not raw.startswith(TOKEN_PREFIX):
        return None
    token = db.execute(
        select(AccessToken).where(AccessToken.token_hash == _hash(raw))
    ).scalar_one_or_none()
    if token and token.expires_at is not None and token.expires_at < time.time():
        return None  # an OAuth access token past its hour; the connector refreshes it
    if token:
        token.last_used_at = dt.datetime.now(dt.UTC)
        db.commit()
    return token


def serialise(token: AccessToken) -> dict:
    return {
        "id": token.id,
        "name": token.name,
        "prefix": token.prefix,
        "scopes": token.scopes,
        "created_at": token.created_at.isoformat() if token.created_at else None,
        "last_used_at": token.last_used_at.isoformat() if token.last_used_at else None,
        # "connector" for apps that signed in with OAuth (claude.ai, ChatGPT)
        "kind": "connector" if token.client_id else "token",
    }


class NewToken(BaseModel):
    name: str
    scopes: list[str] = []


@router.get("")
def list_tokens(db: Session = Depends(get_db)):
    tokens = db.execute(select(AccessToken).order_by(AccessToken.created_at.desc())).scalars()
    return {"tokens": [serialise(t) for t in tokens]}


@router.post("", status_code=201)
def add_token(body: NewToken, db: Session = Depends(get_db)):
    name = body.name.strip()
    if not name:
        raise HTTPException(status_code=400, detail="Give the token a name.")
    unknown = set(body.scopes) - set(WRITE_SCOPES)
    if unknown:
        raise HTTPException(status_code=400, detail=f"Unknown permissions: {sorted(unknown)}")
    token, raw = create_token(db, name[:60], body.scopes)
    db.refresh(token)
    # The only time the token itself is returned
    return {**serialise(token), "token": raw}


@router.delete("/{token_id}", status_code=204)
def revoke_token(token_id: int, db: Session = Depends(get_db)):
    token = db.get(AccessToken, token_id)
    if not token:
        raise HTTPException(status_code=404, detail="Token not found")
    db.delete(token)
    db.commit()
