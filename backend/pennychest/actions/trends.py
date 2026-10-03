"""Month-by-month read actions: money in and out, and net worth over time. The dashboard's
charts call these, as the chat does, so the two always agree."""

from collections import defaultdict
from datetime import date, timedelta
from decimal import Decimal

from pydantic import Field
from sqlalchemy import func, select
from sqlalchemy.orm import Session

from pennychest.accounts.models import Account
from pennychest.actions.common import MONEY_ACCOUNT_TYPES, money
from pennychest.actions.read import group_totals
from pennychest.actions.registry import ActionInput, action
from pennychest.transactions.models import Posting, Transaction


def _month_starts(end: date, months: int) -> list[date]:
    """The first day of each of the `months` calendar months ending with `end`'s."""
    index = end.year * 12 + end.month - 1
    return [date(i // 12, i % 12 + 1, 1) for i in range(index - months + 1, index + 1)]


def _month_end(start: date) -> date:
    following = date(start.year + start.month // 12, start.month % 12 + 1, 1)
    return following - timedelta(days=1)


class CashFlowInput(ActionInput):
    months: int = Field(default=12, ge=1, le=60, description="How many calendar months.")
    end: date | None = Field(
        default=None, description="A day in the last month to include. Defaults to today."
    )


@action(
    """
    Income, spending and what was left over in each calendar month, oldest first, up to and
    including the current month (so far). Spending is positive and refunds reduce it; transfers
    between the user's own accounts are neither. Use this for "am I spending more than I earn?"
    or month-by-month comparisons of money in and out.
    """
)
def cash_flow(db: Session, params: CashFlowInput) -> dict:
    end = params.end or date.today()
    starts = _month_starts(end, params.months)
    income, _, income_total, _ = group_totals(db, starts[0], end, kind="income", group_by="month")
    spent, _, spent_total, unreviewed = group_totals(db, starts[0], end, group_by="month")
    rows = []
    for start in starts:
        key = start.isoformat()[:7]
        rows.append(
            {
                "month": key,
                "income": money(income.get(key)),
                "expenses": money(spent.get(key)),
                "net": money((income.get(key) or 0) - (spent.get(key) or 0)),
            }
        )
    return {
        "months": rows,
        "income_total": money(income_total),
        "expenses_total": money(spent_total),
        "unreviewed_expenses": money(unreviewed),
    }


class NetWorthHistoryInput(ActionInput):
    months: int = Field(default=12, ge=1, le=120, description="How many month ends.")
    end: date | None = Field(
        default=None, description="A day in the last month to include. Defaults to today."
    )


@action(
    """
    Net worth (everything in the user's bank, savings and other asset accounts minus what they
    owe on cards and loans) at the end of each calendar month, oldest first, per currency. The
    last point is today's balance if the month isn't over. Includes transactions not yet
    reviewed.
    """
)
def net_worth_history(db: Session, params: NetWorthHistoryInput) -> dict:
    end = params.end or date.today()
    starts = _month_starts(end, params.months)
    points = [min(_month_end(s), end) for s in starts]
    # Daily totals come from the database; running them up is cheap.
    daily = db.execute(
        select(Account.currency, Transaction.date, func.sum(Posting.amount))
        .join(Posting, Posting.account_id == Account.id)
        .join(Transaction, Posting.transaction_id == Transaction.id)
        .where(Account.type.in_(MONEY_ACCOUNT_TYPES), Transaction.date <= end)
        .group_by(Account.currency, Transaction.date)
        .order_by(Transaction.date)
    ).all()
    by_currency: dict[str, list[tuple[date, Decimal]]] = defaultdict(list)
    for currency, day, amount in daily:
        by_currency[currency].append((day, Decimal(str(amount))))

    series = {}
    for currency, days in sorted(by_currency.items()):
        balance, i, values = Decimal(0), 0, []
        for point in points:
            while i < len(days) and days[i][0] <= point:
                balance += days[i][1]
                i += 1
            values.append({"date": point.isoformat(), "net_worth": money(balance)})
        series[currency] = values
    return {"currencies": series}
