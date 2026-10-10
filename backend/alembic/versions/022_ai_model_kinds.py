"""Share one language model and one decision model between the AI tasks, and remember when an
import was matched with AI

Each task used to have its own provider and model. Existing choices carry over: the decision
model is the first task's that used Jev, the language model chat's (or else the first other
task's), and each task keeps running when it did before.

Revision ID: 022
Revises: 021
Create Date: 2026-10-10

"""

from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa

revision: str = "022"
down_revision: Union[str, None] = "021"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None

TASKS = ("chat", "rules", "categorise", "taps", "insights", "matching", "check_actions")
MODE_TASKS = ("taps", "categorise", "matching", "insights")
DECISION_PROVIDERS = ("typesafe",)

settings = sa.table("app_settings", sa.column("key", sa.String), sa.column("value", sa.Text))


def upgrade() -> None:
    with op.batch_alter_table("import_batches") as batch:
        batch.add_column(sa.Column("matched_at", sa.DateTime(timezone=True), nullable=True))

    conn = op.get_bind()
    stored = dict(conn.execute(sa.select(settings.c.key, settings.c.value)).all())
    new: dict[str, str] = {}
    old_keys = [k for k in stored if k == "ai.categorise.auto"]

    chosen = {}
    for task in TASKS:
        provider = stored.get(f"ai.task.{task}.provider")
        model = stored.get(f"ai.task.{task}.model")
        keys = (f"ai.task.{task}.provider", f"ai.task.{task}.model")
        old_keys += [k for k in keys if k in stored]
        if provider and model:
            chosen[task] = (provider, model)

    for task in TASKS:  # chat first, so its language model wins
        if task not in chosen:
            continue
        provider, model = chosen[task]
        kind = "decision" if provider in DECISION_PROVIDERS else "llm"
        new.setdefault(f"ai.model.{kind}.provider", provider)
        if new[f"ai.model.{kind}.provider"] == provider:
            new.setdefault(f"ai.model.{kind}.model", model)
        new[f"ai.task.{task}.kind"] = kind
        if task in MODE_TASKS:
            automatic = task != "categorise" or stored.get("ai.categorise.auto") != "off"
            new[f"ai.task.{task}.mode"] = "automatic" if automatic else "on_demand"
    # Where some tasks were set up, the others stay off, as they were. Otherwise (as on a new
    # install) they're all on, ready for when a model is chosen.
    if chosen:
        for task in TASKS:
            if task not in chosen:
                new[f"ai.task.{task}.enabled"] = "off"

    if old_keys:
        conn.execute(settings.delete().where(settings.c.key.in_(old_keys)))
    if new:
        conn.execute(settings.insert(), [{"key": k, "value": v} for k, v in new.items()])


def downgrade() -> None:
    with op.batch_alter_table("import_batches") as batch:
        batch.drop_column("matched_at")
    # Each task gets the model of the kind it used
    conn = op.get_bind()
    stored = dict(conn.execute(sa.select(settings.c.key, settings.c.value)).all())
    rows = []
    for task in TASKS:
        kind = stored.get(f"ai.task.{task}.kind")
        if stored.get(f"ai.task.{task}.enabled") == "off" or kind is None:
            continue
        provider = stored.get(f"ai.model.{kind}.provider")
        model = stored.get(f"ai.model.{kind}.model")
        if provider and model:
            rows += [
                {"key": f"ai.task.{task}.provider", "value": provider},
                {"key": f"ai.task.{task}.model", "value": model},
            ]
    if stored.get("ai.task.categorise.mode") == "on_demand":
        rows.append({"key": "ai.categorise.auto", "value": "off"})
    conn.execute(
        settings.delete().where(
            settings.c.key.like("ai.model.%")
            | settings.c.key.like("ai.task.%.kind")
            | settings.c.key.like("ai.task.%.mode")
            | settings.c.key.like("ai.task.%.enabled")
        )
    )
    if rows:
        conn.execute(settings.insert(), rows)
