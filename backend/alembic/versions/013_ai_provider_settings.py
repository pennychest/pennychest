"""Move AI settings to per-provider credentials and per-task model choices

Revision ID: 013
Revises: 012
Create Date: 2026-09-29

"""
from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa

revision: str = "013"
down_revision: Union[str, None] = "012"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None

app_settings = sa.table("app_settings", sa.column("key", sa.String), sa.column("value", sa.Text))

# The model the app used for every Anthropic request before models became selectable.
PREVIOUS_ANTHROPIC_MODEL = "claude-haiku-4-5"
PREVIOUS_OLLAMA_MODEL = "qwen3:1.7b"


def _read(conn) -> dict[str, str]:
    return {
        key: value
        for key, value in conn.execute(sa.select(app_settings.c.key, app_settings.c.value))
        if value
    }


def _write(conn, values: dict[str, str | None]) -> None:
    for key, value in values.items():
        conn.execute(app_settings.delete().where(app_settings.c.key == key))
        if value:
            conn.execute(app_settings.insert().values(key=key, value=value))


def upgrade() -> None:
    conn = op.get_bind()
    old = _read(conn)
    new: dict[str, str | None] = {
        "ai.provider.anthropic.api_key": old.get("anthropic_api_key"),
        # Ollama previously fell back to this address when none was saved.
        "ai.provider.ollama.url": old.get("ollama_url") or (
            "http://ollama:11434" if old.get("ai_provider") == "ollama" else None
        ),
    }
    # Keep whatever was being used before, for both categorising and suggesting rules.
    if old.get("ai_provider") == "ollama":
        choice = ("ollama", old.get("ollama_model") or PREVIOUS_OLLAMA_MODEL)
    elif old.get("anthropic_api_key"):
        choice = ("anthropic", PREVIOUS_ANTHROPIC_MODEL)
    else:
        choice = None
    if choice:
        for task in ("categorise", "rules"):
            new[f"ai.task.{task}.provider"], new[f"ai.task.{task}.model"] = choice
    _write(conn, new)
    old_keys = ("ai_provider", "anthropic_api_key", "ollama_url", "ollama_model")
    _write(conn, {k: None for k in old_keys})


def downgrade() -> None:
    conn = op.get_bind()
    new = _read(conn)
    provider = new.get("ai.task.categorise.provider")
    _write(conn, {
        "anthropic_api_key": new.get("ai.provider.anthropic.api_key"),
        "ollama_url": new.get("ai.provider.ollama.url"),
        "ai_provider": "ollama" if provider == "ollama" else ("claude" if provider else None),
        "ollama_model": new.get("ai.task.categorise.model") if provider == "ollama" else None,
    })
    _write(conn, {k: None for k in new if k.startswith("ai.")})
