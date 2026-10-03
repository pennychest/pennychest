"""Budgets reference their category's account instead of a path string

Revision ID: 014
Revises: 013
Create Date: 2026-09-30

"""

from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa

revision: str = "014"
down_revision: Union[str, None] = "013"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None

accounts = sa.table("accounts", sa.column("id", sa.Integer), sa.column("full_path", sa.String))


def upgrade() -> None:
    conn = op.get_bind()
    # Nothing wrote budgets before this, but keep any rows that match a category.
    old = conn.execute(sa.text("SELECT account_prefix, amount, period_id FROM budgets")).all()
    paths = {
        path: id_ for id_, path in conn.execute(sa.select(accounts.c.id, accounts.c.full_path))
    }
    op.drop_table("budgets")
    budgets = op.create_table(
        "budgets",
        sa.Column("id", sa.Integer, primary_key=True),
        sa.Column(
            "account_id",
            sa.Integer,
            sa.ForeignKey("accounts.id", ondelete="CASCADE"),
            nullable=False,
        ),
        sa.Column("period_id", sa.Integer, sa.ForeignKey("budget_periods.id"), nullable=False),
        sa.Column("amount", sa.Numeric(19, 4), nullable=False),
        sa.Column(
            "created_at", sa.DateTime(timezone=True), nullable=False, server_default=sa.func.now()
        ),
        sa.UniqueConstraint("account_id", "period_id", name="uq_budgets_account_period"),
    )
    rows, seen = [], set()
    for prefix, amount, period_id in old:
        account_id = paths.get(prefix)
        if account_id and (account_id, period_id) not in seen:
            seen.add((account_id, period_id))
            rows.append({"account_id": account_id, "period_id": period_id, "amount": amount})
    if rows:
        op.bulk_insert(budgets, rows)


def downgrade() -> None:
    conn = op.get_bind()
    old = conn.execute(
        sa.text(
            "SELECT a.full_path, b.amount, b.period_id "
            "FROM budgets b JOIN accounts a ON a.id = b.account_id"
        )
    ).all()
    op.drop_table("budgets")
    budgets = op.create_table(
        "budgets",
        sa.Column("id", sa.Integer, primary_key=True),
        sa.Column("account_prefix", sa.String, nullable=False),
        sa.Column("amount", sa.Numeric(19, 4), nullable=False),
        sa.Column("period_id", sa.Integer, sa.ForeignKey("budget_periods.id"), nullable=False),
    )
    if old:
        op.bulk_insert(
            budgets,
            [
                {"account_prefix": path, "amount": amount, "period_id": period_id}
                for path, amount, period_id in old
            ],
        )
