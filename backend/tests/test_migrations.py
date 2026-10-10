from pathlib import Path

import sqlalchemy as sa
from alembic.config import Config

from alembic import command
from pennychest.core.config import settings

BACKEND = Path(__file__).resolve().parents[1]


def _alembic(url, monkeypatch) -> Config:
    monkeypatch.setattr(settings, "database_url", url)
    config = Config(str(BACKEND / "alembic.ini"))
    config.set_main_option("script_location", str(BACKEND / "alembic"))
    return config


def _settings(engine) -> dict[str, str]:
    with engine.connect() as conn:
        return dict(conn.execute(sa.text("SELECT key, value FROM app_settings")).all())


def test_022_carries_each_tasks_model_over(tmp_path, monkeypatch):
    url = f"sqlite:///{tmp_path}/pennychest.db"
    config = _alembic(url, monkeypatch)
    command.upgrade(config, "021")
    engine = sa.create_engine(url)
    old = {
        "ai.task.chat.provider": "openai",
        "ai.task.chat.model": "gpt-5",
        "ai.task.categorise.provider": "anthropic",
        "ai.task.categorise.model": "claude-haiku-4-5",
        "ai.task.taps.provider": "typesafe",
        "ai.task.taps.model": "jev-latest",
        "ai.task.matching.provider": "typesafe",
        "ai.task.matching.model": "jev-latest",
        "ai.categorise.auto": "off",
        "ai.provider.openai.api_key": "sk-1",
    }
    with engine.begin() as conn:
        conn.execute(
            sa.text("INSERT INTO app_settings (key, value) VALUES (:key, :value)"),
            [{"key": k, "value": v} for k, v in old.items()],
        )

    command.upgrade(config, "022")
    new = _settings(engine)
    assert new == {
        "ai.provider.openai.api_key": "sk-1",
        # Chat's language model wins
        "ai.model.llm.provider": "openai",
        "ai.model.llm.model": "gpt-5",
        "ai.model.decision.provider": "typesafe",
        "ai.model.decision.model": "jev-latest",
        "ai.task.chat.kind": "llm",
        "ai.task.categorise.kind": "llm",
        "ai.task.categorise.mode": "on_demand",
        "ai.task.taps.kind": "decision",
        "ai.task.taps.mode": "automatic",
        "ai.task.matching.kind": "decision",
        "ai.task.matching.mode": "automatic",
        # Tasks that weren't set up stay off
        "ai.task.rules.enabled": "off",
        "ai.task.insights.enabled": "off",
        "ai.task.check_actions.enabled": "off",
    }

    command.downgrade(config, "021")
    back = _settings(engine)
    assert back["ai.task.chat.provider"] == "openai"
    assert back["ai.task.categorise.provider"] == "openai"
    assert back["ai.task.taps.provider"] == "typesafe"
    assert back["ai.categorise.auto"] == "off"
    assert not any(k.startswith("ai.model.") for k in back)


def test_022_leaves_new_installs_with_every_task_on(tmp_path, monkeypatch):
    url = f"sqlite:///{tmp_path}/pennychest.db"
    command.upgrade(_alembic(url, monkeypatch), "022")
    assert not any(k.startswith("ai.") for k in _settings(sa.create_engine(url)))
