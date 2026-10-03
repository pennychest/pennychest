"""Add icon column to accounts

Revision ID: 005
Revises: 004
Create Date: 2026-04-17

"""
from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa

revision: str = "005"
down_revision: Union[str, None] = "004"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.add_column(
        "accounts",
        sa.Column("icon", sa.String(), nullable=True),
    )


def downgrade() -> None:
    op.drop_column("accounts", "icon")
