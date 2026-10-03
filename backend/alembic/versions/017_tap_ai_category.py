"""Remember the category an AI model suggested for each card tap

Revision ID: 017
Revises: 016
Create Date: 2026-09-30

"""

from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa

revision: str = "017"
down_revision: Union[str, None] = "016"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    # Batch mode so SQLite can add a foreign-key column; Postgres gets a plain ALTER.
    with op.batch_alter_table("card_taps") as batch:
        batch.add_column(
            sa.Column(
                "ai_account_id",
                sa.Integer,
                sa.ForeignKey(
                    "accounts.id", ondelete="SET NULL", name="card_taps_ai_account_id_fkey"
                ),
                nullable=True,
            )
        )
        batch.add_column(sa.Column("ai_confidence", sa.Float, nullable=True))


def downgrade() -> None:
    with op.batch_alter_table("card_taps") as batch:
        batch.drop_column("ai_confidence")
        batch.drop_column("ai_account_id")
