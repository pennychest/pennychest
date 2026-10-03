"""Add source and description to rules

Revision ID: 007
Revises: 006
Create Date: 2026-05-10

"""
from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa

revision: str = "007"
down_revision: Union[str, None] = "006"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.add_column("rules", sa.Column("source", sa.String(), nullable=True, server_default="manual"))
    op.add_column("rules", sa.Column("description", sa.String(), nullable=True))


def downgrade() -> None:
    op.drop_column("rules", "description")
    op.drop_column("rules", "source")
