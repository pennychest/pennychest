"""Add transfer_peer_id to transactions

Revision ID: 010
Revises: 009
Create Date: 2026-05-24

"""
from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa

revision: str = "010"
down_revision: Union[str, None] = "009"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    # Batch mode so SQLite can add a foreign-key column; Postgres gets a plain ALTER.
    with op.batch_alter_table("transactions") as batch:
        batch.add_column(
            sa.Column(
                "transfer_peer_id",
                sa.Integer,
                sa.ForeignKey(
                    "transactions.id",
                    ondelete="SET NULL",
                    name="transactions_transfer_peer_id_fkey",
                ),
                nullable=True,
            ),
        )
    op.create_index(
        "uq_transfer_peer_id",
        "transactions",
        ["transfer_peer_id"],
        unique=True,
        postgresql_where=sa.text("transfer_peer_id IS NOT NULL"),
        sqlite_where=sa.text("transfer_peer_id IS NOT NULL"),
    )


def downgrade() -> None:
    op.drop_index("uq_transfer_peer_id", table_name="transactions")
    with op.batch_alter_table("transactions") as batch:
        batch.drop_column("transfer_peer_id")
