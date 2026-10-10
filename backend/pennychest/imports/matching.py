"""Matching a statement's new transactions against the ledger where exact matching falls short:
spotting ones already recorded (from an overlapping statement or added by hand), and pairing
up transfers between the user's own accounts whose two sides are described differently.

Exact duplicates are flagged without a model. Everything else needs the "matching" task's model,
straight after the import if it runs automatically or later from the import's review page, and
acts only on answers at or above MIN_PROBABILITY; if the model fails, the import goes ahead
without it (the error is in the AI request logs)."""

import re
from dataclasses import dataclass
from datetime import timedelta

from sqlalchemy import and_, or_, select
from sqlalchemy.orm import Session, aliased

from pennychest.accounts.models import Account
from pennychest.ai.matching import MIN_PROBABILITY, ask_pairs
from pennychest.ai.providers import ProviderError
from pennychest.core.lookup_models import CategorisationSource
from pennychest.transactions.models import Posting, Transaction

DUPLICATE_DAYS = 3
TRANSFER_DAYS = 3

DUPLICATE_QUESTION = (
    "Are {a} and {b} the same bank transaction recorded twice, for example once from a CSV "
    "export and once from a PDF statement, or once added by hand? Descriptions may be "
    "formatted differently. Two separate payments to the same place for the same amount are "
    "not duplicates."
)

TRANSFER_QUESTION = (
    "Are {a} and {b} the two sides of one transfer of money between the user's own accounts, "
    "such as paying off a credit card from a current account or moving money into savings? "
    "Each shows the account it is on and the amount from that account's point of view."
)


@dataclass
class NewRow:
    """A transaction this import created: its bank-account posting and the other side."""

    txn: Transaction
    bank: Posting
    other: Posting


def _normalise(text: str) -> str:
    return " ".join(re.findall(r"[a-z0-9]+", text.lower()))


def _best(pairs: list, probabilities: list[float]) -> dict:
    """For each record, the candidate the model was most confident about, if confident enough.
    `pairs` are (record_key, candidate) in the order they were asked."""
    best: dict = {}
    for (key, candidate), probability in zip(pairs, probabilities):
        if probability >= MIN_PROBABILITY and probability > best.get(key, (None, 0.0))[1]:
            best[key] = (candidate, probability)
    return {key: candidate for key, (candidate, _) in best.items()}


def describe_source(txn: Transaction) -> str:
    if txn.import_batch is None:
        return "added by hand"
    return f"imported from {txn.import_batch.file_name or 'a statement'}"


def flag_duplicates(
    db: Session,
    batch_id: int,
    account_id: int,
    rows: list[NewRow],
    *,
    exact: bool = True,
    use_model: bool = True,
) -> None:
    """Point each new transaction that looks like one already on the same account (from
    another import, or added by hand) at that transaction, for the user to review. `exact`
    flags identical ones; `use_model` asks the matching model about the rest."""
    candidates: dict[int, list[Transaction]] = {}
    for row in rows:
        found = (
            db.execute(
                select(Transaction)
                .join(Posting, Posting.transaction_id == Transaction.id)
                .where(
                    Posting.account_id == account_id,
                    Posting.amount == row.bank.amount,
                    Transaction.id != row.txn.id,
                    or_(
                        Transaction.import_batch_id.is_(None),
                        Transaction.import_batch_id != batch_id,
                    ),
                    Transaction.date >= row.txn.date - timedelta(days=DUPLICATE_DAYS),
                    Transaction.date <= row.txn.date + timedelta(days=DUPLICATE_DAYS),
                )
                .order_by(Transaction.date, Transaction.id)
            )
            .scalars()
            .all()
        )
        if found:
            candidates[row.txn.id] = list(found)

    claimed: set[int] = set()
    ask: list[tuple[NewRow, Transaction]] = []
    for row in rows:
        same = [
            c
            for c in candidates.get(row.txn.id, [])
            if c.date == row.txn.date
            and _normalise(c.description) == _normalise(row.txn.description)
            and c.id not in claimed
        ]
        if same and exact:
            row.txn.duplicate_of_id = same[0].id
            claimed.add(same[0].id)
        elif not same:
            ask += [(row, c) for c in candidates.get(row.txn.id, [])]

    if not ask or not use_model:
        return
    try:
        probabilities = ask_pairs(
            db,
            "duplicate_match",
            DUPLICATE_QUESTION,
            [
                (
                    {"date": r.txn.date.isoformat(), "description": r.txn.description,
                     "source": "this import"},
                    {"date": c.date.isoformat(), "description": c.description,
                     "source": describe_source(c)},
                )
                for r, c in ask
            ],
        )
    except ProviderError:
        return
    best = _best([(r.txn.id, c) for r, c in ask], probabilities)
    for row in rows:
        match = best.get(row.txn.id)
        if match is not None and match.id not in claimed:
            row.txn.duplicate_of_id = match.id
            claimed.add(match.id)


def link_transfers(
    db: Session,
    account_id: int,
    rows: list[NewRow],
    pending_account: Account,
    uncategorised_account: Account,
) -> None:
    """Link new uncategorised transactions to an uncategorised transaction on another of the
    user's accounts for the opposite amount, when the model is confident they're the two sides
    of one transfer. Both are then routed via Transfers:Pending, as linked transfers are."""
    open_accounts = {uncategorised_account.id, pending_account.id}
    rows = [
        r
        for r in rows
        if r.other.account_id in open_accounts
        and r.txn.transfer_peer_id is None
        and r.txn.duplicate_of_id is None
    ]
    if not rows:
        return

    candidate_txn = aliased(Transaction)
    bank_posting = aliased(Posting)
    other_posting = aliased(Posting)
    ask: list[tuple[NewRow, tuple[Transaction, Posting, Posting]]] = []
    for row in rows:
        found = db.execute(
            select(candidate_txn, bank_posting, other_posting)
            .join(bank_posting, bank_posting.transaction_id == candidate_txn.id)
            .join(Account, Account.id == bank_posting.account_id)
            .join(
                other_posting,
                and_(
                    other_posting.transaction_id == candidate_txn.id,
                    other_posting.id != bank_posting.id,
                ),
            )
            .where(
                candidate_txn.id != row.txn.id,
                candidate_txn.transfer_peer_id.is_(None),
                candidate_txn.duplicate_of_id.is_(None),
                candidate_txn.date >= row.txn.date - timedelta(days=TRANSFER_DAYS),
                candidate_txn.date <= row.txn.date + timedelta(days=TRANSFER_DAYS),
                bank_posting.amount == -row.bank.amount,
                bank_posting.account_id != account_id,
                bank_posting.account_id != pending_account.id,
                Account.type.in_(["asset", "liability"]),
                other_posting.account_id.in_(open_accounts),
            )
            .order_by(candidate_txn.date, candidate_txn.id)
        ).all()
        ask += [(row, tuple(f)) for f in found]
    if not ask:
        return

    accounts = {a.id: a.full_path for a in db.execute(select(Account)).scalars()}

    def side(txn: Transaction, bank: Posting) -> dict:
        return {
            "account": accounts.get(bank.account_id),
            "date": txn.date.isoformat(),
            "description": txn.description,
            "amount": str(bank.amount),
        }

    try:
        probabilities = ask_pairs(
            db,
            "transfer_match",
            TRANSFER_QUESTION,
            [(side(r.txn, r.bank), side(c, cb)) for r, (c, cb, _) in ask],
        )
    except ProviderError:
        return
    best = _best([(r.txn.id, found) for r, found in ask], probabilities)
    transfer_source = db.execute(
        select(CategorisationSource).where(CategorisationSource.name == "transfer")
    ).scalar_one()
    for row in rows:
        match = best.get(row.txn.id)
        if match is None:
            continue
        candidate, _, candidate_other = match
        if candidate.transfer_peer_id is not None:
            continue  # claimed by an earlier row
        for posting in (row.other, candidate_other):
            posting.account_id = pending_account.id
            posting.categorised_by_id = transfer_source.id
            posting.rule_id = None
        row.txn.transfer_peer_id = candidate.id
        candidate.transfer_peer_id = row.txn.id
    db.flush()
