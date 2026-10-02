from sqlalchemy import select, text
from sqlalchemy.orm import Session

from pennychest.core.lookup_models import (
    BudgetPeriod,
    CategorisationSource,
    ImportSourceType,
    MatchType,
)


SEED_DATA = {
    BudgetPeriod: ["monthly", "annual"],
    MatchType: ["substring", "prefix", "regex"],
    CategorisationSource: ["rule", "manual", "import_default", "ai", "transfer", "learned"],
    ImportSourceType: ["file", "api"],
}


def seed_lookup_tables(db: Session) -> None:
    for model, values in SEED_DATA.items():
        for value in values:
            existing = db.execute(
                select(model).where(model.name == value)
            ).scalar_one_or_none()
            if not existing:
                db.add(model(name=value))
    db.commit()


ZERO_SUM_TRIGGER_SQL = """
CREATE OR REPLACE FUNCTION check_transaction_balance()
RETURNS TRIGGER AS $$
DECLARE
    txn_id INTEGER;
    balance NUMERIC;
BEGIN
    FOR txn_id IN
        SELECT DISTINCT transaction_id
        FROM postings
        WHERE transaction_id IN (
            SELECT DISTINCT transaction_id FROM new_postings_table
        )
    LOOP
        SELECT COALESCE(SUM(amount), 0) INTO balance
        FROM postings
        WHERE transaction_id = txn_id;

        IF balance != 0 THEN
            RAISE EXCEPTION 'Transaction % does not balance: sum = %', txn_id, balance;
        END IF;
    END LOOP;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS trg_check_balance ON postings;
CREATE CONSTRAINT TRIGGER trg_check_balance
    AFTER INSERT OR UPDATE OR DELETE ON postings
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION check_transaction_balance();
"""

# Simpler per-row deferred trigger
ZERO_SUM_TRIGGER_SIMPLE_SQL = """
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

DROP TRIGGER IF EXISTS trg_check_balance ON postings;
CREATE CONSTRAINT TRIGGER trg_check_balance
    AFTER INSERT OR UPDATE OR DELETE ON postings
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION check_transaction_balance();
"""


def create_zero_sum_trigger(db: Session) -> None:
    db.execute(text(ZERO_SUM_TRIGGER_SIMPLE_SQL))
    db.commit()
