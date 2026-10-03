"""Flag imported transactions that look like one already in the ledger

Revision ID: 020
Revises: 019
Create Date: 2026-10-02

"""

from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa

revision: str = "020"
down_revision: Union[str, None] = "019"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    # Batch mode so SQLite can add a foreign-key column; Postgres gets a plain ALTER.
    with op.batch_alter_table("transactions") as batch:
        batch.add_column(
            sa.Column(
                "duplicate_of_id",
                sa.Integer,
                sa.ForeignKey(
                    "transactions.id",
                    ondelete="SET NULL",
                    name="transactions_duplicate_of_id_fkey",
                ),
                nullable=True,
            )
        )


def downgrade() -> None:
    with op.batch_alter_table("transactions") as batch:
        batch.drop_column("duplicate_of_id")
