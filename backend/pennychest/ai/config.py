"""Stored AI settings: provider credentials, the one language model and one decision model
the tasks share, how each task is set up, and each provider's last-fetched model list.
Everything lives in the app_settings table."""

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
    "csv_columns": "Reading CSV columns",
}

# The two kinds of model. A language model (LLM) writes text and can do anything; a decision
# model (such as Jev) only picks from fixed answers, with a confidence, but is fast and cheap.
LLM = "llm"
DECISION = "decision"
KIND_LABELS = {LLM: "language model", DECISION: "decision model"}

# Which kinds of model can do each task. Rules and chat need to write text.
TASK_KINDS = {
    "taps": (DECISION, LLM),
    "categorise": (DECISION, LLM),
    "rules": (LLM,),
    "chat": (LLM,),
    "check_actions": (DECISION, LLM),
    "matching": (DECISION, LLM),
    "insights": (DECISION, LLM),
    "csv_columns": (DECISION, LLM),
}

# Tasks that can run by themselves whenever new data arrives, or only when the user asks.
# Rules only run when asked; chat and checking chat's actions run whenever the chat does.
AUTOMATIC = "automatic"
ON_DEMAND = "on_demand"
MODE_TASKS = {"taps", "categorise", "matching", "insights", "csv_columns"}


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


def get_model(db: Session, kind: str) -> tuple[str | None, str | None]:
    """The provider and model chosen for a kind of model."""
    return _get(db, f"ai.model.{kind}.provider"), _get(db, f"ai.model.{kind}.model")


def set_model(db: Session, kind: str, provider_id: str | None, model: str | None) -> None:
    _put(db, f"ai.model.{kind}.provider", provider_id)
    _put(db, f"ai.model.{kind}.model", model if provider_id else None)
    db.commit()


def _model_chosen(db: Session, kind: str) -> bool:
    return all(get_model(db, kind))


def task_enabled(db: Session, task: str) -> bool:
    """On unless the user turns it off. It only runs once a model of its kind is chosen."""
    return _get(db, f"ai.task.{task}.enabled") != "off"


def set_task_enabled(db: Session, task: str, enabled: bool) -> None:
    _put(db, f"ai.task.{task}.enabled", "on" if enabled else "off")
    db.commit()


def task_kind(db: Session, task: str) -> str:
    """The kind of model a task uses: the user's choice, or else a decision model when one is
    chosen, as it's faster and cheaper."""
    kinds = TASK_KINDS[task]
    stored = _get(db, f"ai.task.{task}.kind")
    if stored in kinds:
        return stored
    return next((k for k in kinds if _model_chosen(db, k)), kinds[0])


def set_task_kind(db: Session, task: str, kind: str) -> None:
    _put(db, f"ai.task.{task}.kind", kind)
    db.commit()


def task_mode(db: Session, task: str) -> str | None:
    """Whether a task runs by itself on new data or only when asked; None for tasks without
    the choice. Unless the user chooses, decision models run automatically, being fast and
    cheap, and language models only when asked."""
    if task not in MODE_TASKS:
        return None
    stored = _get(db, f"ai.task.{task}.mode")
    if stored in (AUTOMATIC, ON_DEMAND):
        return stored
    return AUTOMATIC if task_kind(db, task) == DECISION else ON_DEMAND


def set_task_mode(db: Session, task: str, mode: str) -> None:
    _put(db, f"ai.task.{task}.mode", mode)
    db.commit()


def get_task(db: Session, task: str) -> tuple[str | None, str | None]:
    """The provider and model a task would use, or (None, None) if it's turned off."""
    if not task_enabled(db, task):
        return None, None
    return get_model(db, task_kind(db, task))


def task_available(db: Session, task: str) -> bool:
    """Whether the task is on and has a model, so it can run when asked."""
    return all(get_task(db, task))


def runs_automatically(db: Session, task: str) -> bool:
    """Whether the task should run by itself as new data arrives."""
    return task_available(db, task) and task_mode(db, task) != ON_DEMAND


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
