"""Add chat conversations and messages

Revision ID: 015
Revises: 014
Create Date: 2026-09-30

"""
from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa
from sqlalchemy.dialects.postgresql import JSONB

JSON_TYPE = sa.JSON().with_variant(JSONB(), "postgresql")

revision: str = "015"
down_revision: Union[str, None] = "014"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.create_table(
        "chat_conversations",
        sa.Column("id", sa.Integer, primary_key=True),
        sa.Column("title", sa.String, nullable=False),
        sa.Column(
            "created_at", sa.DateTime(timezone=True), nullable=False, server_default=sa.func.now()
        ),
        sa.Column(
            "updated_at", sa.DateTime(timezone=True), nullable=False, server_default=sa.func.now()
        ),
    )
    op.create_table(
        "chat_messages",
        sa.Column("id", sa.Integer, primary_key=True),
        sa.Column(
            "conversation_id",
            sa.Integer,
            sa.ForeignKey("chat_conversations.id", ondelete="CASCADE"),
            nullable=False,
        ),
        sa.Column("role", sa.String, nullable=False),
        sa.Column("content", sa.Text, nullable=True),
        sa.Column("tool_calls", JSON_TYPE, nullable=True),
        sa.Column("tool_call_id", sa.String, nullable=True),
        sa.Column("tool_name", sa.String, nullable=True),
        sa.Column("is_error", sa.Boolean, nullable=False, server_default=sa.false()),
        sa.Column("raw", JSON_TYPE, nullable=True),
        sa.Column("raw_provider", sa.String, nullable=True),
        sa.Column(
            "created_at", sa.DateTime(timezone=True), nullable=False, server_default=sa.func.now()
        ),
    )
    op.create_index("ix_chat_messages_conversation_id", "chat_messages", ["conversation_id"])


def downgrade() -> None:
    op.drop_index("ix_chat_messages_conversation_id", table_name="chat_messages")
    op.drop_table("chat_messages")
    op.drop_table("chat_conversations")
