from datetime import date, timedelta

from fastapi import APIRouter, Depends, Query
from sqlalchemy import func, select, and_, case
from sqlalchemy.orm import Session

from pennychest.accounts.models import Account
from pennychest.core.database import get_db
from pennychest.transactions.models import Posting, Transaction

router = APIRouter(prefix="/api/reports", tags=["reports"])


@router.get("/summary")
def get_summary(
    date_from: date | None = None,
    date_to: date | None = None,
    db: Session = Depends(get_db),
):
    if not date_from:
        today = date.today()
        date_from = today.replace(day=1)
    if not date_to:
        date_to = date.today()

    # Net worth: assets - liabilities (all confirmed transactions)
    net_worth_query = (
        select(
            Account.currency,
            func.coalesce(func.sum(
                case(
                    (Account.type.in_(["asset"]), Posting.amount),
                    (Account.type.in_(["liability"]), Posting.amount),
                    else_=0,
                )
            ), 0).label("net_worth"),
        )
        .select_from(Posting)
        .join(Account, Posting.account_id == Account.id)
        .join(Transaction, Posting.transaction_id == Transaction.id)
        .where(Transaction.status == "confirmed")
        .where(Account.type.in_(["asset", "liability"]))
        .group_by(Account.currency)
    )
    net_worth_rows = db.execute(net_worth_query).all()
    net_worth = {row.currency: float(row.net_worth) for row in net_worth_rows}

    # Income this period
    income_query = (
        select(func.coalesce(func.sum(func.abs(Posting.amount)), 0))
        .select_from(Posting)
        .join(Account, Posting.account_id == Account.id)
        .join(Transaction, Posting.transaction_id == Transaction.id)
        .where(
            and_(
                Transaction.status == "confirmed",
                Account.type == "income",
                Transaction.date >= date_from,
                Transaction.date <= date_to,
            )
        )
    )
    income = float(db.execute(income_query).scalar() or 0)

    # Expenses this period
    expenses_query = (
        select(func.coalesce(func.sum(Posting.amount), 0))
        .select_from(Posting)
        .join(Account, Posting.account_id == Account.id)
        .join(Transaction, Posting.transaction_id == Transaction.id)
        .where(
            and_(
                Transaction.status == "confirmed",
                Account.type == "expense",
                Transaction.date >= date_from,
                Transaction.date <= date_to,
            )
        )
    )
    expenses = float(db.execute(expenses_query).scalar() or 0)

    savings_rate = ((income - expenses) / income * 100) if income > 0 else 0

    return {
        "net_worth": net_worth,
        "income": income,
        "expenses": expenses,
        "savings_rate": round(savings_rate, 1),
        "date_from": date_from.isoformat(),
        "date_to": date_to.isoformat(),
    }


@router.get("/expenses-by-category")
def expenses_by_category(
    date_from: date | None = None,
    date_to: date | None = None,
    parent_path: str = "Expenses",
    db: Session = Depends(get_db),
):
    if not date_from:
        today = date.today()
        date_from = today.replace(day=1)
    if not date_to:
        date_to = date.today()

    query = (
        select(
            Account.full_path,
            Account.name,
            func.sum(Posting.amount).label("total"),
        )
        .select_from(Posting)
        .join(Account, Posting.account_id == Account.id)
        .join(Transaction, Posting.transaction_id == Transaction.id)
        .where(
            and_(
                Transaction.status == "confirmed",
                Account.full_path.like(f"{parent_path}:%"),
                Transaction.date >= date_from,
                Transaction.date <= date_to,
            )
        )
        .group_by(Account.full_path, Account.name)
        .order_by(func.sum(Posting.amount).desc())
    )
    rows = db.execute(query).all()
    return [
        {"full_path": row.full_path, "name": row.name, "total": float(row.total)}
        for row in rows
    ]
