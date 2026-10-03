from datetime import date, timedelta
from decimal import Decimal

from sqlalchemy import case, func, or_, select
from sqlalchemy.orm import Session

from pennychest.accounts.models import Account
from pennychest.transactions.models import Posting, Transaction

PERIODS = ("monthly", "annual")
CENTS = Decimal("0.01")


def period_bounds(period: str, on: date) -> tuple[date, date]:
    """The calendar month or year containing `on`, as [start, end)."""
    if period == "monthly":
        start = on.replace(day=1)
        end = (start + timedelta(days=32)).replace(day=1)
    else:
        start = date(on.year, 1, 1)
        end = date(on.year + 1, 1, 1)
    return start, end


def _money(value) -> Decimal:
    # SQLite returns sums as floats; go through str to avoid binary rounding noise.
    return Decimal(str(value or 0)).quantize(CENTS)


def _in_category(account: Account):
    """The category itself or any of its subcategories, matched on the current path so
    renames are followed."""
    return or_(
        Account.full_path == account.full_path,
        Account.full_path.startswith(f"{account.full_path}:", autoescape=True),
    )


def category_spend(
    db: Session, account: Account, start: date, end: date
) -> tuple[Decimal, Decimal]:
    """Total spent in [start, end) and how much of that is in unreviewed transactions.
    Refunds (negative postings) reduce the total."""
    total, unreviewed = db.execute(
        select(
            func.coalesce(func.sum(Posting.amount), 0),
            func.coalesce(
                func.sum(case((Transaction.status == "pending", Posting.amount), else_=0)), 0
            ),
        )
        .join(Account, Posting.account_id == Account.id)
        .join(Transaction, Posting.transaction_id == Transaction.id)
        .where(_in_category(account), Transaction.date >= start, Transaction.date < end)
    ).one()
    return _money(total), _money(unreviewed)


def suggest_amount(db: Session, account: Account, period: str, on: date) -> dict:
    """What the category has cost recently: the average of the last three full months for a
    monthly budget, or the last twelve full months combined for an annual one."""
    this_month = on.replace(day=1)
    months = []
    end = this_month
    for _ in range(3 if period == "monthly" else 12):
        start = (end - timedelta(days=1)).replace(day=1)
        spent, _ = category_spend(db, account, start, end)
        months.append({"month": start.isoformat()[:7], "spent": spent})
        end = start
    months.reverse()
    total = sum((m["spent"] for m in months), Decimal(0))
    amount = total / 3 if period == "monthly" else total
    return {"suggested": amount.quantize(CENTS), "months": months}
