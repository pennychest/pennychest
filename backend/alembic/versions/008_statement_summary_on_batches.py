"""Per-statement summary fields on import_batches

Adds period_start, period_end, opening_balance and closing_balance to
import_batches so we can record the statement-level metadata extracted from
HSBC PDFs. These power the opening-balance recommendation flow on import
and the future "statement coverage gaps" view.

Revision ID: 008
Revises: 007
Create Date: 2026-05-10

"""
from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa

revision: str = "008"
down_revision: Union[str, None] = "007"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.add_column("import_batches", sa.Column("period_start", sa.Date(), nullable=True))
    op.add_column("import_batches", sa.Column("period_end", sa.Date(), nullable=True))
    op.add_column("import_batches", sa.Column("opening_balance", sa.Numeric(19, 4), nullable=True))
    op.add_column("import_batches", sa.Column("closing_balance", sa.Numeric(19, 4), nullable=True))


def downgrade() -> None:
    op.drop_column("import_batches", "closing_balance")
    op.drop_column("import_batches", "opening_balance")
    op.drop_column("import_batches", "period_end")
    op.drop_column("import_batches", "period_start")
