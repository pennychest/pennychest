"""Add currency column to transactions

Revision ID: 004
Revises: 003
Create Date: 2026-04-16

"""
from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa

revision: str = "004"
down_revision: Union[str, None] = "003"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.add_column(
        "transactions",
        sa.Column("currency", sa.String(), nullable=False, server_default="GBP"),
    )


def downgrade() -> None:
    op.drop_column("transactions", "currency")
