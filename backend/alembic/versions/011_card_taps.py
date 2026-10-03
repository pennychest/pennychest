"""Add card taps and wallet card pairings

Revision ID: 011
Revises: 010
Create Date: 2026-09-26

"""
from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa

revision: str = "011"
down_revision: Union[str, None] = "010"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.create_table(
        "wallet_cards",
        sa.Column("id", sa.Integer, primary_key=True),
        sa.Column("card_name", sa.String, nullable=False, unique=True),
        sa.Column(
            "account_id",
            sa.Integer,
            sa.ForeignKey("accounts.id", ondelete="CASCADE"),
            nullable=False,
        ),
        sa.Column(
            "created_at", sa.DateTime(timezone=True), nullable=False, server_default=sa.func.now()
        ),
    )
    op.create_table(
        "card_taps",
        sa.Column("id", sa.Integer, primary_key=True),
        sa.Column("tapped_on", sa.Date, nullable=False),
        sa.Column("merchant", sa.String, nullable=False),
        sa.Column("amount", sa.Numeric(19, 4), nullable=False),
        sa.Column("currency", sa.String, nullable=False, server_default="GBP"),
        sa.Column("card_name", sa.String, nullable=True),
        sa.Column(
            "transaction_id",
            sa.Integer,
            sa.ForeignKey("transactions.id", ondelete="SET NULL"),
            nullable=True,
            unique=True,
        ),
        sa.Column("dismissed", sa.Boolean, nullable=False, server_default=sa.false()),
        sa.Column(
            "created_at", sa.DateTime(timezone=True), nullable=False, server_default=sa.func.now()
        ),
    )
    op.create_index("ix_card_taps_card_name", "card_taps", ["card_name"])


def downgrade() -> None:
    op.drop_index("ix_card_taps_card_name", table_name="card_taps")
    op.drop_table("card_taps")
    op.drop_table("wallet_cards")
