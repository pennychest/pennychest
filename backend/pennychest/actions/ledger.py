"""Actions that change the ledger itself. All need the `transactions` scope.

Imported transactions can be changed or deleted like any other; the statement rows they came
from stay with their import, so what the bank reported is never lost. The descriptions steer
models towards changing an imported transaction's category rather than its bank details.
"""

import datetime as dt
from decimal import Decimal

from pydantic import Field
from sqlalchemy import select
from sqlalchemy.orm import Session, joinedload

from pennychest.accounts.models import Account, AccountType
from pennychest.actions.common import (
    CATEGORY_TYPES,
    MONEY_ACCOUNT_TYPES,
    describe_transaction,
    find_account,
    find_category,
)
from pennychest.actions.registry import ActionError, ActionInput, Scope, action
from pennychest.core.lookup_models import CategorisationSource
from pennychest.imports.models import RawImportRow
from pennychest.taps.models import CardTap
from pennychest.transactions.models import Posting, Transaction
from pennychest.transactions.service import remove_transaction

IDS = Field(min_length=1, max_length=200, description="IDs from list_transactions.")


def _source_id(db: Session) -> int:
    return db.execute(
        select(CategorisationSource.id).where(CategorisationSource.name == "ai")
    ).scalar_one()


def _money_account(db: Session, ref: str) -> Account:
    return find_account(db, ref, MONEY_ACCOUNT_TYPES, what="account")


def _category_amount(category: Account, amount: Decimal) -> Decimal:
    """Spending is recorded as a positive amount against an expense category and income as
    a negative amount against an income category; the bank or card side is the opposite."""
    return amount if category.type == AccountType.EXPENSE else -amount


def _load(db: Session, ids: list[int]) -> dict[int, Transaction]:
    return {
        t.id: t
        for t in db.execute(
            select(Transaction)
            .where(Transaction.id.in_(ids))
            .options(joinedload(Transaction.postings).joinedload(Posting.account))
        )
        .unique()
        .scalars()
    }


def _get(db: Session, transaction_id: int) -> Transaction:
    txn = _load(db, [transaction_id]).get(transaction_id)
    if not txn:
        raise ActionError(f"There is no transaction {transaction_id}.")
    return txn


def snapshot(db: Session, txn: Transaction) -> dict:
    """Everything needed to put a transaction back exactly as it was, including what pointed
    at it (statement rows, card taps and a transfer partner)."""
    return {
        "id": txn.id,
        "date": txn.date.isoformat(),
        "description": txn.description,
        "status": txn.status,
        "currency": txn.currency,
        "import_batch_id": txn.import_batch_id,
        "import_metadata": txn.import_metadata,
        "transfer_peer_id": txn.transfer_peer_id,
        "postings": [
            {
                "id": p.id,
                "account_id": p.account_id,
                "amount": str(p.amount),
                "categorised_by_id": p.categorised_by_id,
                "rule_id": p.rule_id,
            }
            for p in txn.postings
        ],
        "raw_row_ids": list(
            db.execute(
                select(RawImportRow.id).where(RawImportRow.transaction_id == txn.id)
            ).scalars()
        ),
        "tap_ids": list(
            db.execute(select(CardTap.id).where(CardTap.transaction_id == txn.id)).scalars()
        ),
        "peer_of": list(
            db.execute(
                select(Transaction.id).where(Transaction.transfer_peer_id == txn.id)
            ).scalars()
        ),
    }


def _reload(db: Session, txn_id: int) -> dict:
    db.expire_all()
    return describe_transaction(_get(db, txn_id))


class AddTransactionInput(ActionInput):
    description: str = Field(min_length=1, max_length=200, description='e.g. "Coffee at Pret".')
    amount: Decimal = Field(
        description="How much was spent (for a spending category) or received (for an income "
        "category). Use a negative amount for a refund."
    )
    category: str = Field(description="Spending or income category (path or name).")
    account: str = Field(
        description="The bank account, card or cash account it was paid from or into."
    )
    date: dt.date | None = Field(default=None, description="Defaults to today.")


@action(
    """
    Record a payment or income the user tells you about, such as a cash purchase. It is saved
    as reviewed. Use add_transfer for money moved between the user's own accounts.
    """,
    scope=Scope.TRANSACTIONS,
)
def add_transaction(db: Session, params: AddTransactionInput) -> dict:
    if params.amount == 0:
        raise ActionError("The amount can't be zero.")
    category = find_category(db, params.category)
    account = _money_account(db, params.account)
    amount = _category_amount(category, params.amount)
    source_id = _source_id(db)
    txn = Transaction(
        date=params.date or dt.date.today(),
        description=params.description.strip(),
        status="confirmed",
        currency=account.currency,
    )
    db.add(txn)
    db.flush()
    db.add_all(
        [
            Posting(
                transaction_id=txn.id,
                account_id=category.id,
                amount=amount,
                categorised_by_id=source_id,
            ),
            Posting(
                transaction_id=txn.id,
                account_id=account.id,
                amount=-amount,
                categorised_by_id=source_id,
            ),
        ]
    )
    db.commit()
    added = _reload(db, txn.id)
    return {
        "added": added,
        "_change": {
            "summary": f'Added "{added["description"]}" (£{added["amount"]}, {added["category"]}) '
            f"on {added['date']}",
            "undo": {"transaction_id": txn.id},
        },
    }


class AddTransferInput(ActionInput):
    amount: Decimal = Field(gt=0, description="How much was moved.")
    from_account: str = Field(description="Account the money left (path or name).")
    to_account: str = Field(description="Account the money arrived in (path or name).")
    description: str = Field(default="Transfer", min_length=1, max_length=200)
    date: dt.date | None = Field(default=None, description="Defaults to today.")


@action(
    """
    Record money moved between two of the user's own accounts, such as paying off a credit
    card or moving money into savings. Transfers aren't spending, so they have no category.
    """,
    scope=Scope.TRANSACTIONS,
)
def add_transfer(db: Session, params: AddTransferInput) -> dict:
    source = _money_account(db, params.from_account)
    destination = _money_account(db, params.to_account)
    if source.id == destination.id:
        raise ActionError("The two accounts must be different.")
    source_id = _source_id(db)
    txn = Transaction(
        date=params.date or dt.date.today(),
        description=params.description.strip(),
        status="confirmed",
        currency=source.currency,
    )
    db.add(txn)
    db.flush()
    db.add_all(
        [
            Posting(
                transaction_id=txn.id,
                account_id=source.id,
                amount=-params.amount,
                categorised_by_id=source_id,
            ),
            Posting(
                transaction_id=txn.id,
                account_id=destination.id,
                amount=params.amount,
                categorised_by_id=source_id,
            ),
        ]
    )
    db.commit()
    return {
        "added": _reload(db, txn.id),
        "_change": {
            "summary": f"Added a £{params.amount} transfer from {source.full_path} to "
            f"{destination.full_path}",
            "undo": {"transaction_id": txn.id},
        },
    }


class EditTransactionInput(ActionInput):
    transaction_id: int
    description: str | None = Field(default=None, min_length=1, max_length=200)
    date: dt.date | None = None
    amount: Decimal | None = Field(
        default=None,
        description="New amount, meaning the same as in add_transaction (or the amount moved, "
        "for a transfer).",
    )
    category: str | None = Field(default=None, description="New category (path or name).")
    account: str | None = Field(
        default=None, description="New bank account or card it was paid from or into."
    )


@action(
    """
    Change a transaction's description, date, amount, category or account. Imported
    transactions ("imported": true) came from the user's bank statement: to fix how they're
    categorised change only the category, and only change their amount, date, account or
    description when the user asks for that specifically. Transfers can change amount, date
    and description but have no category.
    """,
    scope=Scope.TRANSACTIONS,
)
def edit_transaction(db: Session, params: EditTransactionInput) -> dict:
    txn = _get(db, params.transaction_id)
    if params.amount is not None and params.amount == 0:
        raise ActionError("The amount can't be zero.")
    before = snapshot(db, txn)
    changed = [
        name
        for name in ("description", "date", "amount", "category", "account")
        if getattr(params, name) is not None
    ]

    categories = [p for p in txn.postings if p.account.type in CATEGORY_TYPES]
    money_side = [p for p in txn.postings if p.account.type in MONEY_ACCOUNT_TYPES]
    simple = len(categories) == 1 and len(money_side) == 1 and len(txn.postings) == 2
    transfer = not categories and len(money_side) == 2

    if params.description is not None:
        txn.description = params.description.strip()
    if params.date is not None:
        txn.date = params.date

    if params.category is not None or params.account is not None:
        if not simple:
            raise ActionError(
                "Only transactions with one category and one account can have their category "
                "or account changed here."
            )
        category_posting, money_posting = categories[0], money_side[0]
        if params.category is not None:
            new_category = find_category(db, params.category)
            if new_category.type != category_posting.account.type:
                # Moving between spending and income flips which way the money counts.
                spent = _category_amount(category_posting.account, category_posting.amount)
                category_posting.amount = _category_amount(new_category, spent)
                money_posting.amount = -category_posting.amount
            category_posting.account_id = new_category.id
            category_posting.account = new_category
            category_posting.categorised_by_id = _source_id(db)
            category_posting.rule_id = None
        if params.account is not None:
            new_account = _money_account(db, params.account)
            money_posting.account_id = new_account.id
            money_posting.account = new_account

    if params.amount is not None:
        if simple:
            category_posting, money_posting = categories[0], money_side[0]
            category_posting.amount = _category_amount(category_posting.account, params.amount)
            money_posting.amount = -category_posting.amount
        elif transfer:
            if params.amount < 0:
                raise ActionError("A transfer's amount must be positive.")
            outgoing, incoming = sorted(money_side, key=lambda p: p.amount)
            outgoing.amount, incoming.amount = -params.amount, params.amount
        else:
            raise ActionError("This transaction is split, so its amount can't be changed here.")

    db.commit()
    edited = _reload(db, txn.id)
    result = {"edited": edited}
    if changed:
        result["_change"] = {
            "summary": f'Edited the {", ".join(changed)} of "{before["description"]}" '
            f"({before['date']})",
            "undo": {"before": before},
        }
    return result


class TransactionIdsInput(ActionInput):
    transaction_ids: list[int] = IDS


@action(
    """
    Delete transactions, including transfers. Imported transactions ("imported": true) came
    from the user's bank statement, so only delete one when the user asks for it
    specifically, for example a duplicate; to fix how one is categorised, change its
    category instead.
    """,
    scope=Scope.TRANSACTIONS,
)
def delete_transactions(db: Session, params: TransactionIdsInput) -> dict:
    ids = list(dict.fromkeys(params.transaction_ids))
    txns = _load(db, ids)
    deleted, skipped, snapshots = [], [], []
    for txn_id in ids:
        txn = txns.get(txn_id)
        if not txn:
            skipped.append({"id": txn_id, "reason": "not found"})
        else:
            deleted.append(describe_transaction(txn))
            snapshots.append(snapshot(db, txn))
            remove_transaction(db, txn)
    db.commit()
    result = {"deleted": deleted, "skipped": skipped}
    if deleted:
        what = (
            f'"{deleted[0]["description"]}" ({deleted[0]["date"]})'
            if len(deleted) == 1
            else f"{len(deleted)} transactions"
        )
        result["_change"] = {"summary": f"Deleted {what}", "undo": {"transactions": snapshots}}
    return result


@action(
    """
    Mark imported transactions as reviewed, meaning the user has checked their category.
    """,
    scope=Scope.TRANSACTIONS,
)
def mark_reviewed(db: Session, params: TransactionIdsInput) -> dict:
    ids = list(dict.fromkeys(params.transaction_ids))
    txns = _load(db, ids)
    reviewed, skipped = [], []
    for txn_id in ids:
        txn = txns.get(txn_id)
        if not txn:
            skipped.append({"id": txn_id, "reason": "not found"})
        elif txn.status == "confirmed":
            skipped.append({"id": txn_id, "reason": "already reviewed"})
        else:
            txn.status = "confirmed"
            reviewed.append(txn_id)
    db.commit()
    result = {"reviewed": reviewed, "skipped": skipped}
    if reviewed:
        count = f"{len(reviewed)} transaction{'s' if len(reviewed) != 1 else ''}"
        result["_change"] = {
            "summary": f"Marked {count} as reviewed",
            "undo": {"transaction_ids": reviewed},
        }
    return result
