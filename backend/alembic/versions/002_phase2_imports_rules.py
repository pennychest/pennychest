"""Phase 2: imports and rules enhancements

Revision ID: 002
Revises: 001
Create Date: 2026-04-13

"""
from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa

revision: str = "002"
down_revision: Union[str, None] = "001"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


# Batch mode so SQLite can add foreign-key columns (it rebuilds the table);
# Postgres still gets a plain ALTER TABLE.
def upgrade() -> None:
    # Add rule_id to postings (tracks which rule categorised this posting)
    with op.batch_alter_table("postings") as batch:
        batch.add_column(
            sa.Column(
                "rule_id",
                sa.Integer,
                sa.ForeignKey("rules.id", ondelete="SET NULL", name="postings_rule_id_fkey"),
                nullable=True,
            ),
        )

    # Add import_batch_id to transactions (links transaction to its import batch)
    with op.batch_alter_table("transactions") as batch:
        batch.add_column(
            sa.Column(
                "import_batch_id",
                sa.Integer,
                sa.ForeignKey(
                    "import_batches.id",
                    ondelete="SET NULL",
                    name="transactions_import_batch_id_fkey",
                ),
                nullable=True,
            ),
        )

    # Add file_name and account_id to import_batches
    with op.batch_alter_table("import_batches") as batch:
        batch.add_column(sa.Column("file_name", sa.String, nullable=True))
        batch.add_column(
            sa.Column(
                "account_id",
                sa.Integer,
                sa.ForeignKey("accounts.id", name="import_batches_account_id_fkey"),
                nullable=True,
            ),
        )


def downgrade() -> None:
    with op.batch_alter_table("import_batches") as batch:
        batch.drop_column("account_id")
        batch.drop_column("file_name")
    with op.batch_alter_table("transactions") as batch:
        batch.drop_column("import_batch_id")
    with op.batch_alter_table("postings") as batch:
        batch.drop_column("rule_id")
