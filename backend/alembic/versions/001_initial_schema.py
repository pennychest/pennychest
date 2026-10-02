"""Initial schema

Revision ID: 001
Revises:
Create Date: 2026-04-13

"""
from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa

revision: str = "001"
down_revision: Union[str, None] = None
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def _is_postgres() -> bool:
    return op.get_bind().dialect.name == "postgresql"


def upgrade() -> None:
    # Create enum type using raw SQL so it is not re-emitted by create_table.
    if _is_postgres():
        op.execute(
            "CREATE TYPE account_type AS ENUM "
            "('asset', 'liability', 'income', 'expense', 'equity')"
        )

    # Lookup tables
    op.create_table(
        "budget_periods",
        sa.Column("id", sa.Integer, primary_key=True),
        sa.Column("name", sa.String, nullable=False, unique=True),
    )
    op.create_table(
        "match_types",
        sa.Column("id", sa.Integer, primary_key=True),
        sa.Column("name", sa.String, nullable=False, unique=True),
    )
    op.create_table(
        "categorisation_sources",
        sa.Column("id", sa.Integer, primary_key=True),
        sa.Column("name", sa.String, nullable=False, unique=True),
    )
    op.create_table(
        "import_source_types",
        sa.Column("id", sa.Integer, primary_key=True),
        sa.Column("name", sa.String, nullable=False, unique=True),
    )

    # Core tables — use sa.Text for the enum column to avoid SQLAlchemy
    # re-emitting CREATE TYPE during create_table; the DB column is typed
    # account_type via the server_default cast in raw SQL above.
    op.create_table(
        "accounts",
        sa.Column("id", sa.Integer, primary_key=True),
        sa.Column("name", sa.String, nullable=False),
        sa.Column("full_path", sa.String, nullable=False, unique=True),
        sa.Column("parent_id", sa.Integer, sa.ForeignKey("accounts.id"), nullable=True),
        sa.Column("type", sa.Text, nullable=False),
        sa.Column("currency", sa.String, nullable=False, server_default="GBP"),
    )
    op.create_table(
        "transactions",
        sa.Column("id", sa.Integer, primary_key=True),
        sa.Column("date", sa.Date, nullable=False),
        sa.Column("description", sa.String, nullable=False),
        sa.Column("status", sa.String, nullable=False, server_default="pending"),
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False, server_default=sa.func.now()),
        sa.CheckConstraint("status IN ('pending', 'confirmed')", name="ck_status"),
    )
    op.create_table(
        "postings",
        sa.Column("id", sa.Integer, primary_key=True),
        sa.Column("transaction_id", sa.Integer, sa.ForeignKey("transactions.id"), nullable=False),
        sa.Column("account_id", sa.Integer, sa.ForeignKey("accounts.id"), nullable=False),
        sa.Column("amount", sa.Numeric(19, 4), nullable=False),
        sa.Column("categorised_by_id", sa.Integer, sa.ForeignKey("categorisation_sources.id"), nullable=False),
    )

    # Import tables
    op.create_table(
        "import_batches",
        sa.Column("id", sa.Integer, primary_key=True),
        sa.Column("importer_name", sa.String, nullable=False),
        sa.Column("source_type_id", sa.Integer, sa.ForeignKey("import_source_types.id"), nullable=False),
        sa.Column("file_path", sa.String, nullable=True),
        sa.Column("file_hash", sa.String, nullable=True),
        sa.Column("imported_at", sa.DateTime(timezone=True), nullable=False, server_default=sa.func.now()),
    )
    op.create_table(
        "raw_import_rows",
        sa.Column("id", sa.Integer, primary_key=True),
        sa.Column("batch_id", sa.Integer, sa.ForeignKey("import_batches.id"), nullable=False),
        sa.Column("raw_content", sa.String, nullable=False),
        sa.Column("line_number", sa.Integer, nullable=True),
        sa.Column("transaction_id", sa.Integer, sa.ForeignKey("transactions.id"), nullable=True),
    )

    # Rule engine
    op.create_table(
        "rules",
        sa.Column("id", sa.Integer, primary_key=True),
        sa.Column("pattern", sa.String, nullable=False),
        sa.Column("match_type_id", sa.Integer, sa.ForeignKey("match_types.id"), nullable=False),
        sa.Column("target_account_id", sa.Integer, sa.ForeignKey("accounts.id"), nullable=False),
        sa.Column("priority", sa.Integer, nullable=False, server_default="0"),
    )

    # Budgets
    op.create_table(
        "budgets",
        sa.Column("id", sa.Integer, primary_key=True),
        sa.Column("account_prefix", sa.String, nullable=False),
        sa.Column("amount", sa.Numeric(19, 4), nullable=False),
        sa.Column("period_id", sa.Integer, sa.ForeignKey("budget_periods.id"), nullable=False),
    )

    # Zero-sum deferred constraint trigger. Postgres only: SQLite has no deferred
    # triggers, and the API rejects unbalanced transactions on either database.
    if _is_postgres():
        op.execute("""
        CREATE OR REPLACE FUNCTION check_transaction_balance()
        RETURNS TRIGGER AS $$
        DECLARE
            balance NUMERIC;
            txn_id INTEGER;
        BEGIN
            IF TG_OP = 'DELETE' THEN
                txn_id := OLD.transaction_id;
            ELSE
                txn_id := NEW.transaction_id;
            END IF;

            SELECT COALESCE(SUM(amount), 0) INTO balance
            FROM postings
            WHERE transaction_id = txn_id;

            IF balance != 0 THEN
                RAISE EXCEPTION 'Transaction % does not balance: sum = %', txn_id, balance;
            END IF;

            RETURN NULL;
        END;
        $$ LANGUAGE plpgsql;

        CREATE CONSTRAINT TRIGGER trg_check_balance
            AFTER INSERT OR UPDATE OR DELETE ON postings
            DEFERRABLE INITIALLY DEFERRED
            FOR EACH ROW
            EXECUTE FUNCTION check_transaction_balance();
        """)


def downgrade() -> None:
    if _is_postgres():
        op.execute("DROP TRIGGER IF EXISTS trg_check_balance ON postings")
        op.execute("DROP FUNCTION IF EXISTS check_transaction_balance()")
    op.drop_table("budgets")
    op.drop_table("rules")
    op.drop_table("raw_import_rows")
    op.drop_table("import_batches")
    op.drop_table("postings")
    op.drop_table("transactions")
    op.drop_table("accounts")
    op.drop_table("import_source_types")
    op.drop_table("categorisation_sources")
    op.drop_table("match_types")
    op.drop_table("budget_periods")
    sa.Enum(name="account_type").drop(op.get_bind(), checkfirst=True)
