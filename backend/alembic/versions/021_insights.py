"""Remember what kind of merchant each recurring payment is, and how unusual each charge is

Revision ID: 021
Revises: 020
Create Date: 2026-10-02

"""

from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa

revision: str = "021"
down_revision: Union[str, None] = "020"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.create_table(
        "merchant_labels",
        sa.Column("merchant_key", sa.String, primary_key=True),
        sa.Column("kind", sa.String, nullable=False),
        sa.Column("confidence", sa.Float, nullable=True),
        sa.Column(
            "updated_at", sa.DateTime(timezone=True), nullable=False, server_default=sa.func.now()
        ),
        sa.CheckConstraint("kind IN ('subscription', 'bill', 'other')", name="ck_merchant_kind"),
    )
    with op.batch_alter_table("transactions") as batch:
        batch.add_column(sa.Column("unusual_score", sa.Float, nullable=True))


def downgrade() -> None:
    with op.batch_alter_table("transactions") as batch:
        batch.drop_column("unusual_score")
    op.drop_table("merchant_labels")
