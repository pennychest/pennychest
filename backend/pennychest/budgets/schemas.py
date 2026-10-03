from datetime import date
from decimal import Decimal
from typing import Literal

from pydantic import BaseModel, Field

Period = Literal["monthly", "annual"]


class BudgetCreate(BaseModel):
    account_id: int
    amount: Decimal = Field(gt=0)
    period: Period = "monthly"


class BudgetUpdate(BaseModel):
    amount: Decimal | None = Field(default=None, gt=0)
    period: Period | None = None


class BudgetResponse(BaseModel):
    id: int
    account_id: int
    account_full_path: str
    account_icon: str | None
    currency: str
    period: Period
    period_start: date
    period_end: date
    amount: Decimal
    spent: Decimal
    unreviewed: Decimal
    remaining: Decimal


class MonthSpend(BaseModel):
    month: str
    spent: Decimal


class BudgetSuggestion(BaseModel):
    suggested: Decimal
    months: list[MonthSpend]
