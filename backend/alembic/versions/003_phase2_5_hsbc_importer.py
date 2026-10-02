"""Phase 2.5: HSBC importer — bank_identifier on accounts, metadata on transactions

Revision ID: 003
Revises: 002
Create Date: 2026-04-14

"""
from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa
from sqlalchemy.dialects.postgresql import JSONB

JSON_TYPE = sa.JSON().with_variant(JSONB(), "postgresql")

revision: str = "003"
down_revision: Union[str, None] = "002"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    # Add bank_identifier to accounts for importer auto-selection
    # Stores sort code + account number (e.g. "12-34-56 12345678")
    # or masked card number (e.g. "****1234") for credit cards
    op.add_column(
        "accounts",
        sa.Column("bank_identifier", sa.String, nullable=True),
    )

    # Add import_metadata JSONB to transactions for importer-specific fields
    # e.g. payment_type ("DD", "FP"), payment_method ("contactless"), received_date
    op.add_column(
        "transactions",
        sa.Column("import_metadata", JSON_TYPE, nullable=True),
    )


def downgrade() -> None:
    op.drop_column("transactions", "import_metadata")
    op.drop_column("accounts", "bank_identifier")
