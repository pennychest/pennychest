"""Record changes made through actions so they can be undone

Revision ID: 016
Revises: 015
Create Date: 2026-09-30

"""
from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa
from sqlalchemy.dialects.postgresql import JSONB

JSON_TYPE = sa.JSON().with_variant(JSONB(), "postgresql")

revision: str = "016"
down_revision: Union[str, None] = "015"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.create_table(
        "action_changes",
        sa.Column("id", sa.Integer, primary_key=True),
        sa.Column(
            "created_at", sa.DateTime(timezone=True), nullable=False, server_default=sa.func.now()
        ),
        sa.Column("action", sa.String, nullable=False),
        sa.Column("arguments", JSON_TYPE, nullable=True),
        sa.Column("summary", sa.Text, nullable=False),
        sa.Column("undo", JSON_TYPE, nullable=False),
        sa.Column("source", sa.String, nullable=False),
        sa.Column(
            "conversation_id",
            sa.Integer,
            sa.ForeignKey("chat_conversations.id", ondelete="SET NULL"),
            nullable=True,
        ),
        sa.Column(
            "chat_message_id",
            sa.Integer,
            sa.ForeignKey("chat_messages.id", ondelete="SET NULL"),
            nullable=True,
        ),
        sa.Column("undone_at", sa.DateTime(timezone=True), nullable=True),
    )


def downgrade() -> None:
    op.drop_table("action_changes")
