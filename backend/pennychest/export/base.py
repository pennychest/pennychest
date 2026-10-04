"""BaseExporter interface for plugin-based exports.

Exporters are discovered via Python entry points:
    importlib.metadata.entry_points(group='pennychest.exporters')

An exporter turns the ledger into a file in another format. It gets a Ledger: a read-only
snapshot in plain Python types, so exporters don't depend on PennyChest's database models
and the schema can change without breaking them. The database backup is not an exporter:
it's a copy of PennyChest itself, so it stays in core.
"""

from abc import ABC, abstractmethod
from dataclasses import dataclass
from datetime import date
from decimal import Decimal

from sqlalchemy import select
from sqlalchemy.orm import Session, joinedload

from pennychest.accounts.models import Account
from pennychest.budgets.models import Budget
from pennychest.core.lookup_models import BudgetPeriod
from pennychest.core.plugins import load_plugins
from pennychest.transactions.models import Transaction

ENTRY_POINT_GROUP = "pennychest.exporters"


@dataclass(frozen=True)
class LedgerAccount:
    id: int
    name: str
    full_path: str  # e.g. "Expenses:Food:Groceries"
    type: str  # "asset" | "liability" | "income" | "expense" | "equity"
    currency: str
    parent_id: int | None
    bank_identifier: str | None  # e.g. "12-34-56 12345678" or "****1234"


@dataclass(frozen=True)
class LedgerPosting:
    id: int
    account_id: int
    amount: Decimal  # Signed; a transaction's postings sum to zero


@dataclass(frozen=True)
class LedgerTransaction:
    id: int
    date: date
    description: str
    status: str  # "confirmed" | "pending" (imported, still to be reviewed)
    currency: str
    postings: tuple[LedgerPosting, ...]  # In the order they were created


@dataclass(frozen=True)
class LedgerBudget:
    account_id: int  # Covers this account and everything below it
    amount: Decimal
    period: str  # "monthly" | "annual"
    created_on: date | None


@dataclass(frozen=True)
class Ledger:
    """Everything an exporter can see, as of `today`."""

    today: date
    accounts: tuple[LedgerAccount, ...]  # Parents before their children
    transactions: tuple[LedgerTransaction, ...]  # Oldest first
    budgets: tuple[LedgerBudget, ...]


def load_ledger(db: Session, today: date) -> Ledger:
    accounts = db.execute(select(Account).order_by(Account.full_path)).scalars()
    transactions = (
        db.execute(
            select(Transaction)
            .options(joinedload(Transaction.postings))
            .order_by(Transaction.date, Transaction.id)
        )
        .unique()
        .scalars()
    )
    budgets = db.execute(
        select(Budget, BudgetPeriod.name)
        .join(BudgetPeriod, BudgetPeriod.id == Budget.period_id)
        .order_by(Budget.id)
    ).all()
    return Ledger(
        today=today,
        accounts=tuple(
            LedgerAccount(
                id=a.id,
                name=a.name,
                full_path=a.full_path,
                type=a.type.value,
                currency=a.currency or "GBP",
                parent_id=a.parent_id,
                bank_identifier=a.bank_identifier,
            )
            for a in accounts
        ),
        transactions=tuple(
            LedgerTransaction(
                id=t.id,
                date=t.date,
                description=t.description or "",
                status=t.status,
                currency=t.currency or "GBP",
                postings=tuple(
                    LedgerPosting(id=p.id, account_id=p.account_id, amount=Decimal(p.amount))
                    for p in sorted(t.postings, key=lambda p: p.id)
                ),
            )
            for t in transactions
        ),
        budgets=tuple(
            LedgerBudget(
                account_id=b.account_id,
                amount=Decimal(b.amount),
                period=period,
                created_on=b.created_at.date() if b.created_at else None,
            )
            for b, period in budgets
        ),
    )


class BaseExporter(ABC):
    """Abstract base class for all exporters."""

    name: str = ""
    # Shown on the export page, e.g. "Beancount ledger"
    label: str = ""
    description: str = ""
    # The downloaded file is named pennychest-<date>.<file_extension>
    file_extension: str = ""
    media_type: str = "application/octet-stream"

    @abstractmethod
    def export(self, ledger: Ledger) -> str | bytes:
        """The ledger in this exporter's format."""


def discover_exporters() -> dict[str, BaseExporter]:
    """Discover all registered exporters via entry points."""
    return load_plugins(ENTRY_POINT_GROUP)


def get_exporter(name: str) -> BaseExporter | None:
    return discover_exporters().get(name)
