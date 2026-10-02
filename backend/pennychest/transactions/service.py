from sqlalchemy import update
from sqlalchemy.orm import Session

from pennychest.imports.models import RawImportRow
from pennychest.transactions.models import Transaction


def remove_transaction(db: Session, txn: Transaction) -> None:
    """Delete a transaction. The statement rows it was imported from stay with their import
    as the record of what the bank reported; they just stop pointing at it. Card taps and
    transfer pairs are unlinked by their foreign keys."""
    db.execute(
        update(RawImportRow)
        .where(RawImportRow.transaction_id == txn.id)
        .values(transaction_id=None)
    )
    db.delete(txn)
