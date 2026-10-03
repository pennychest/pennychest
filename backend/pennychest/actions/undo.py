"""Reversing changes recorded by run_action. Each undo checks that the data still looks the
way the change left it, and refuses with an explanation rather than guessing if it doesn't."""

import datetime as dt
from collections.abc import Callable
from decimal import Decimal

from sqlalchemy import select, update
from sqlalchemy.orm import Session

from pennychest.accounts.models import Account
from pennychest.actions.models import ActionChange
from pennychest.actions.registry import ActionError
from pennychest.actions.write import move_account
from pennychest.budgets.models import Budget
from pennychest.imports.models import ImportBatch, RawImportRow
from pennychest.rules.models import Rule
from pennychest.taps.models import CardTap
from pennychest.transactions.models import Posting, Transaction
from pennychest.transactions.service import remove_transaction


class UndoError(ActionError):
    """A change that can't be undone as things stand."""


def _undo_create_category(db: Session, data: dict) -> None:
    account = db.get(Account, data["account_id"])
    if not account:
        raise UndoError("That category has already been deleted.")
    uses = [
        what
        for what, query in (
            ("transactions", select(Posting.id).where(Posting.account_id == account.id)),
            ("subcategories", select(Account.id).where(Account.parent_id == account.id)),
            ("rules", select(Rule.id).where(Rule.target_account_id == account.id)),
            ("budgets", select(Budget.id).where(Budget.account_id == account.id)),
        )
        if db.execute(query.limit(1)).first()
    ]
    if uses:
        raise UndoError(f"{account.full_path} now has {' and '.join(uses)}, so it wasn't removed.")
    db.delete(account)


def _undo_rename_category(db: Session, data: dict) -> None:
    account = db.get(Account, data["account_id"])
    if not account:
        raise UndoError("That category has since been deleted.")
    if account.full_path != data["to_path"]:
        raise UndoError(f"It has been renamed again since (it's now {account.full_path}).")
    taken = db.execute(select(Account.id).where(Account.full_path == data["from_path"])).first()
    if taken:
        raise UndoError(f"Another category is now called {data['from_path']}.")
    move_account(db, account, data["from_path"], data["from_name"])


def _undo_create_rule(db: Session, data: dict) -> None:
    rule = db.get(Rule, data["rule_id"])
    if not rule:
        raise UndoError("That rule has already been deleted.")
    db.delete(rule)


def _budget(db: Session, data: dict) -> Budget | None:
    return db.execute(
        select(Budget).where(
            Budget.account_id == data["account_id"], Budget.period_id == data["period_id"]
        )
    ).scalar_one_or_none()


def _undo_set_budget(db: Session, data: dict) -> None:
    budget = _budget(db, data)
    if data["previous"] is None:
        if budget:
            db.delete(budget)
    elif budget:
        budget.amount = Decimal(data["previous"])
    else:
        db.add(
            Budget(
                account_id=data["account_id"],
                period_id=data["period_id"],
                amount=Decimal(data["previous"]),
            )
        )


def _undo_delete_budget(db: Session, data: dict) -> None:
    if not db.get(Account, data["account_id"]):
        raise UndoError("That category has since been deleted.")
    if _budget(db, data):
        raise UndoError("That category has a budget for this period again.")
    db.add(
        Budget(
            account_id=data["account_id"],
            period_id=data["period_id"],
            amount=Decimal(data["amount"]),
        )
    )


def _undo_recategorise(db: Session, data: dict) -> None:
    restored = 0
    for before in data["postings"]:
        posting = db.get(Posting, before["id"])
        if posting and db.get(Account, before["account_id"]):
            posting.account_id = before["account_id"]
            posting.categorised_by_id = before["categorised_by_id"]
            posting.rule_id = before["rule_id"]
            restored += 1
    if not restored:
        raise UndoError("Those transactions have since been deleted.")


def _undo_add(db: Session, data: dict) -> None:
    txn = db.get(Transaction, data["transaction_id"])
    if not txn:
        raise UndoError("That transaction has already been deleted.")
    remove_transaction(db, txn)


def _undo_edit(db: Session, data: dict) -> None:
    before = data["before"]
    txn = db.get(Transaction, before["id"])
    if not txn:
        raise UndoError("That transaction has since been deleted.")
    postings = {p.id: p for p in txn.postings}
    if set(postings) != {p["id"] for p in before["postings"]}:
        raise UndoError("That transaction has been split or rebuilt since.")
    txn.date = dt.date.fromisoformat(before["date"])
    txn.description = before["description"]
    for p in before["postings"]:
        posting = postings[p["id"]]
        posting.account_id = p["account_id"]
        posting.amount = Decimal(p["amount"])
        posting.categorised_by_id = p["categorised_by_id"]
        posting.rule_id = p["rule_id"]


def _undo_delete_transactions(db: Session, data: dict) -> None:
    snapshots = data["transactions"]
    for snap in snapshots:
        if db.get(Transaction, snap["id"]):
            raise UndoError(f"Transaction {snap['id']} already exists again.")
        missing = [
            p["account_id"] for p in snap["postings"] if not db.get(Account, p["account_id"])
        ]
        if missing:
            raise UndoError("An account one of the transactions used has since been deleted.")
    for snap in snapshots:
        batch_id = snap["import_batch_id"]
        peer_id = snap["transfer_peer_id"]
        db.add(
            Transaction(
                id=snap["id"],
                date=dt.date.fromisoformat(snap["date"]),
                description=snap["description"],
                status=snap["status"],
                currency=snap["currency"],
                import_batch_id=batch_id if batch_id and db.get(ImportBatch, batch_id) else None,
                import_metadata=snap["import_metadata"],
                transfer_peer_id=peer_id if peer_id and db.get(Transaction, peer_id) else None,
            )
        )
        db.flush()
        db.add_all(
            [
                Posting(
                    id=p["id"],
                    transaction_id=snap["id"],
                    account_id=p["account_id"],
                    amount=Decimal(p["amount"]),
                    categorised_by_id=p["categorised_by_id"],
                    rule_id=p["rule_id"] if p["rule_id"] and db.get(Rule, p["rule_id"]) else None,
                )
                for p in snap["postings"]
            ]
        )
        # Reconnect whatever pointed at it, if nothing else has claimed it since.
        db.execute(
            update(RawImportRow)
            .where(RawImportRow.id.in_(snap["raw_row_ids"]), RawImportRow.transaction_id.is_(None))
            .values(transaction_id=snap["id"])
        )
        db.execute(
            update(CardTap)
            .where(CardTap.id.in_(snap["tap_ids"]), CardTap.transaction_id.is_(None))
            .values(transaction_id=snap["id"])
        )
        db.execute(
            update(Transaction)
            .where(Transaction.id.in_(snap["peer_of"]), Transaction.transfer_peer_id.is_(None))
            .values(transfer_peer_id=snap["id"])
        )


def _undo_mark_reviewed(db: Session, data: dict) -> None:
    db.execute(
        update(Transaction)
        .where(Transaction.id.in_(data["transaction_ids"]))
        .values(status="pending")
    )


UNDO: dict[str, Callable[[Session, dict], None]] = {
    "create_category": _undo_create_category,
    "rename_category": _undo_rename_category,
    "create_rule": _undo_create_rule,
    "set_budget": _undo_set_budget,
    "delete_budget": _undo_delete_budget,
    "recategorise_transactions": _undo_recategorise,
    "add_transaction": _undo_add,
    "add_transfer": _undo_add,
    "edit_transaction": _undo_edit,
    "delete_transactions": _undo_delete_transactions,
    "mark_reviewed": _undo_mark_reviewed,
}


def undo_change(db: Session, change: ActionChange) -> None:
    if change.undone_at is not None:
        raise UndoError("This change has already been undone.")
    # Each undo checks everything before changing anything, so a refusal leaves no trace.
    UNDO[change.action](db, change.undo)
    change.undone_at = dt.datetime.now(dt.UTC)
    db.commit()
