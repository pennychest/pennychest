"""Stored AI settings: provider credentials, the provider/model chosen per task, and
each provider's last-fetched model list. Everything lives in the app_settings table."""

import json
from datetime import UTC, datetime

from sqlalchemy.orm import Session

from pennychest.settings.models import AppSetting

TASKS = {
    "taps": "Categorising card taps",
    "categorise": "Categorising transactions",
    "rules": "Suggesting rules",
    "chat": "Chat",
    "check_actions": "Checking chat actions",
    "matching": "Matching taps, duplicates and transfers",
    "insights": "Flagging subscriptions and unusual charges",
}

# Jobs that run by themselves on every new piece of data, rather than when the user asks.
AUTOMATIC_TASKS = {"taps", "categorise", "check_actions", "matching", "insights"}


def _get(db: Session, key: str) -> str | None:
    setting = db.get(AppSetting, key)
    return setting.value if setting else None


def _put(db: Session, key: str, value: str | None) -> None:
    setting = db.get(AppSetting, key)
    if value is None:
        if setting:
            db.delete(setting)
    elif setting:
        setting.value = value
    else:
        db.add(AppSetting(key=key, value=value))


def provider_value(db: Session, provider_id: str, field: str) -> str | None:
    return _get(db, f"ai.provider.{provider_id}.{field}")


def set_provider_values(db: Session, provider_id: str, values: dict[str, str | None]) -> None:
    for field, value in values.items():
        cleaned = value.strip() if isinstance(value, str) else None
        _put(db, f"ai.provider.{provider_id}.{field}", cleaned or None)
    # New credentials haven't been tried yet
    _put(db, f"ai.provider_status.{provider_id}", None)
    db.commit()


def get_provider_problem(db: Session, provider_id: str) -> str | None:
    """Why the provider's credentials failed the last time they were tried, if they did."""
    return _get(db, f"ai.provider_status.{provider_id}")


def set_provider_problem(db: Session, provider_id: str, problem: str | None) -> None:
    _put(db, f"ai.provider_status.{provider_id}", problem)
    db.commit()


def get_task(db: Session, task: str) -> tuple[str | None, str | None]:
    return _get(db, f"ai.task.{task}.provider"), _get(db, f"ai.task.{task}.model")


def set_task(db: Session, task: str, provider_id: str | None, model: str | None) -> None:
    _put(db, f"ai.task.{task}.provider", provider_id)
    _put(db, f"ai.task.{task}.model", model if provider_id else None)
    db.commit()


def cached_models(db: Session, provider_id: str) -> tuple[list[dict], str | None]:
    raw = _get(db, f"ai.provider.{provider_id}.models")
    updated_at = _get(db, f"ai.provider.{provider_id}.models_updated_at")
    return (json.loads(raw) if raw else []), updated_at


def cache_models(db: Session, provider_id: str, models: list[dict]) -> str:
    updated_at = datetime.now(UTC).isoformat(timespec="seconds")
    _put(db, f"ai.provider.{provider_id}.models", json.dumps(models))
    _put(db, f"ai.provider.{provider_id}.models_updated_at", updated_at)
    db.commit()
    return updated_at


CHAT_SCOPE_KEY = "ai.chat.scopes"
CHAT_SCOPES = ("organise", "transactions")


def get_chat_scopes(db: Session) -> list[str]:
    """What the chat may change beyond looking things up. Nothing unless the user allows it."""
    raw = _get(db, CHAT_SCOPE_KEY)
    stored = json.loads(raw) if raw else []
    return [s for s in CHAT_SCOPES if s in stored]


def set_chat_scopes(db: Session, scopes: list[str]) -> None:
    _put(db, CHAT_SCOPE_KEY, json.dumps([s for s in CHAT_SCOPES if s in scopes]))
    db.commit()


AUTO_CATEGORISE_KEY = "ai.categorise.auto"


def get_auto_categorise(db: Session) -> bool:
    """Whether imports are categorised straight away. On unless the user turns it off; it
    only runs once a provider and model are chosen for categorising."""
    return _get(db, AUTO_CATEGORISE_KEY) != "off"


def set_auto_categorise(db: Session, enabled: bool) -> None:
    _put(db, AUTO_CATEGORISE_KEY, "on" if enabled else "off")
    db.commit()
