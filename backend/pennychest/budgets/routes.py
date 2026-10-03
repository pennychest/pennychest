from datetime import date, timedelta

from fastapi import APIRouter, Depends, HTTPException, Query
from sqlalchemy import select
from sqlalchemy.orm import Session, joinedload

from pennychest.accounts.models import Account, AccountType
from pennychest.budgets.models import Budget
from pennychest.budgets.schemas import (
    BudgetCreate,
    BudgetResponse,
    BudgetSuggestion,
    BudgetUpdate,
    Period,
)
from pennychest.budgets.service import category_spend, period_bounds, suggest_amount
from pennychest.core.database import get_db
from pennychest.core.lookup_models import BudgetPeriod

router = APIRouter(prefix="/api/budgets", tags=["budgets"])


def _period_id(db: Session, name: str) -> int:
    return db.execute(select(BudgetPeriod.id).where(BudgetPeriod.name == name)).scalar_one()


def _expense_account(db: Session, account_id: int) -> Account:
    account = db.get(Account, account_id)
    if not account:
        raise HTTPException(status_code=404, detail="Category not found")
    if account.type != AccountType.EXPENSE:
        raise HTTPException(
            status_code=400, detail="Budgets can only be set on spending categories"
        )
    return account


def _to_response(db: Session, budget: Budget, on: date) -> BudgetResponse:
    period = budget.period.name
    start, end = period_bounds(period, on)
    spent, unreviewed = category_spend(db, budget.account, start, end)
    return BudgetResponse(
        id=budget.id,
        account_id=budget.account_id,
        account_full_path=budget.account.full_path,
        account_icon=budget.account.icon,
        currency=budget.account.currency,
        period=period,
        period_start=start,
        period_end=end - timedelta(days=1),
        amount=budget.amount,
        spent=spent,
        unreviewed=unreviewed,
        remaining=budget.amount - spent,
    )


def _get_budget(db: Session, budget_id: int) -> Budget:
    budget = db.get(Budget, budget_id)
    if not budget:
        raise HTTPException(status_code=404, detail="Budget not found")
    return budget


def _ensure_unique(db: Session, account_id: int, period_id: int, exclude_id: int | None = None):
    query = select(Budget.id).where(Budget.account_id == account_id, Budget.period_id == period_id)
    if exclude_id is not None:
        query = query.where(Budget.id != exclude_id)
    if db.execute(query).first():
        raise HTTPException(
            status_code=409, detail="That category already has a budget for this period"
        )


@router.get("", response_model=list[BudgetResponse])
def list_budgets(
    on: date | None = Query(default=None, description="Any date in the period to report on"),
    db: Session = Depends(get_db),
):
    on = on or date.today()
    budgets = (
        db.execute(select(Budget).options(joinedload(Budget.account), joinedload(Budget.period)))
        .scalars()
        .all()
    )
    responses = [_to_response(db, b, on) for b in budgets]
    return sorted(responses, key=lambda r: (r.period != "monthly", r.account_full_path))


@router.get("/suggestion", response_model=BudgetSuggestion)
def budget_suggestion(
    account_id: int,
    period: Period = "monthly",
    on: date | None = None,
    db: Session = Depends(get_db),
):
    """What this category has cost recently, as a starting point for a budget."""
    account = _expense_account(db, account_id)
    return suggest_amount(db, account, period, on or date.today())


@router.post("", response_model=BudgetResponse, status_code=201)
def create_budget(body: BudgetCreate, db: Session = Depends(get_db)):
    account = _expense_account(db, body.account_id)
    period_id = _period_id(db, body.period)
    _ensure_unique(db, account.id, period_id)
    budget = Budget(account_id=account.id, period_id=period_id, amount=body.amount)
    db.add(budget)
    db.commit()
    db.refresh(budget)
    return _to_response(db, budget, date.today())


@router.patch("/{budget_id}", response_model=BudgetResponse)
def update_budget(budget_id: int, body: BudgetUpdate, db: Session = Depends(get_db)):
    budget = _get_budget(db, budget_id)
    if body.period is not None:
        period_id = _period_id(db, body.period)
        _ensure_unique(db, budget.account_id, period_id, exclude_id=budget.id)
        budget.period_id = period_id
    if body.amount is not None:
        budget.amount = body.amount
    db.commit()
    db.refresh(budget)
    return _to_response(db, budget, date.today())


@router.delete("/{budget_id}", status_code=204)
def delete_budget(budget_id: int, db: Session = Depends(get_db)):
    db.delete(_get_budget(db, budget_id))
    db.commit()
