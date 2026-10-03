"""Add sign-in sessions

Revision ID: 012
Revises: 011
Create Date: 2026-09-27

"""
from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa

revision: str = "012"
down_revision: Union[str, None] = "011"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.create_table(
        "auth_sessions",
        sa.Column("id", sa.Integer, primary_key=True),
        sa.Column("token_hash", sa.String, nullable=False, unique=True),
        sa.Column(
            "created_at", sa.DateTime(timezone=True), nullable=False, server_default=sa.func.now()
        ),
        sa.Column("expires_at", sa.BigInteger, nullable=False),
    )


def downgrade() -> None:
    op.drop_table("auth_sessions")
