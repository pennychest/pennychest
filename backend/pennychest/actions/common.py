import re
from decimal import Decimal

from sqlalchemy import select
from sqlalchemy.orm import Session

from pennychest.accounts.models import Account, AccountType
from pennychest.actions.registry import ActionError
from pennychest.budgets.service import CENTS

CATEGORY_TYPES = (AccountType.EXPENSE, AccountType.INCOME)
MONEY_ACCOUNT_TYPES = (AccountType.ASSET, AccountType.LIABILITY)
UNCATEGORISED_PATH = "Expenses:Uncategorised"


def money(value) -> str:
    """Amounts as strings with two decimal places, which read unambiguously to a model."""
    return str(Decimal(str(value or 0)).quantize(CENTS))


def path_segment(name: str) -> str:
    """The path segment for a display name, matching how the Accounts page builds paths:
    words joined, non-alphanumerics dropped ("Amazon Prime" -> "AmazonPrime")."""
    return "".join(re.sub(r"[^A-Za-z0-9]", "", word) for word in name.split())


def find_account(
    db: Session, ref: str, types: tuple[AccountType, ...] | None = None, what: str = "account"
) -> Account:
    """Look an account up by full path ("Expenses:Food:Groceries") or, if unambiguous, by its
    last path segment or name ("Groceries"), case-insensitively."""
    ref = ref.strip()
    accounts = db.execute(select(Account)).scalars().all()
    if types:
        accounts = [a for a in accounts if a.type in types]
    wanted = ref.lower()
    exact = [a for a in accounts if a.full_path.lower() == wanted]
    if exact:
        return exact[0]
    squashed = path_segment(ref).lower()
    matches = [
        a
        for a in accounts
        if a.name.lower() == wanted or a.full_path.rsplit(":", 1)[-1].lower() in (wanted, squashed)
    ]
    if len(matches) == 1:
        return matches[0]
    if matches:
        options = ", ".join(sorted(a.full_path for a in matches))
        raise ActionError(f"{ref!r} matches several {what}s: {options}. Use the full path.")
    raise ActionError(
        f"No {what} called {ref!r}. Use list_categories or list_accounts to see them."
    )


def find_category(db: Session, ref: str) -> Account:
    return find_account(db, ref, CATEGORY_TYPES, what="category")


def in_account(account: Account):
    """SQL condition for the account itself or anything beneath it."""
    return (Account.full_path == account.full_path) | Account.full_path.startswith(
        f"{account.full_path}:", autoescape=True
    )


def describe_transaction(txn) -> dict:
    """A transaction as actions report it: the amount categorised (positive for spending,
    negative for income or refunds) and the bank account or card on the other side."""
    categories = [p for p in txn.postings if p.account.type in CATEGORY_TYPES]
    paid_with = [p for p in txn.postings if p.account.type in MONEY_ACCOUNT_TYPES]
    return {
        "id": txn.id,
        "date": txn.date.isoformat(),
        "description": txn.description,
        "category": ", ".join(p.account.full_path for p in categories) or None,
        "amount": money(sum((p.amount for p in categories), Decimal(0))),
        "account": ", ".join(p.account.full_path for p in paid_with) or None,
        "status": txn.status,
        "imported": txn.import_batch_id is not None,
    }
