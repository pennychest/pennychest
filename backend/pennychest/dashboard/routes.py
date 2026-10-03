"""Each person's dashboard: which widgets it shows, in what order, with what settings. Saved
on the server so it's the same on every device."""

import json
from typing import Literal

from fastapi import APIRouter, Depends
from pydantic import BaseModel, Field
from sqlalchemy.orm import Session

from pennychest.core.database import get_db
from pennychest.settings.models import AppSetting

router = APIRouter(prefix="/api/dashboard", tags=["dashboard"])

LAYOUT_KEY = "dashboard.layout"

WidgetType = Literal[
    "stat",
    "spending_by_category",
    "spending_trend",
    "income_vs_expenses",
    "net_worth",
    "budgets",
    "top_merchants",
    "pending",
    "unmatched_taps",
    "subscriptions",
]
Period = Literal["this_month", "last_month", "last_3_months", "last_12_months", "this_year"]


class WidgetSettings(BaseModel):
    model_config = {"extra": "forbid"}

    # stat tiles
    stat: Literal["net_worth", "income", "expenses", "savings_rate"] | None = None
    # widgets over a period
    period: Period | None = None
    # month-by-month charts
    months: int | None = Field(default=None, ge=3, le=36)
    chart: Literal["bar", "donut", "line"] | None = None
    # spending trend: categories to plot as their own series (none = all spending)
    categories: list[str] | None = Field(default=None, max_length=8)


class Widget(BaseModel):
    model_config = {"extra": "forbid"}

    id: str = Field(min_length=1, max_length=40)
    type: WidgetType
    settings: WidgetSettings = Field(default_factory=WidgetSettings)


class Layout(BaseModel):
    widgets: list[Widget] = Field(max_length=40)


DEFAULT_LAYOUT = Layout(
    widgets=[
        Widget(id="net-worth", type="stat", settings=WidgetSettings(stat="net_worth")),
        Widget(id="income", type="stat", settings=WidgetSettings(stat="income")),
        Widget(id="expenses", type="stat", settings=WidgetSettings(stat="expenses")),
        Widget(id="savings-rate", type="stat", settings=WidgetSettings(stat="savings_rate")),
        Widget(
            id="by-category",
            type="spending_by_category",
            settings=WidgetSettings(period="this_month"),
        ),
        Widget(id="pending", type="pending"),
        Widget(id="in-out", type="income_vs_expenses", settings=WidgetSettings(months=12)),
    ]
)


@router.get("/layout", response_model=Layout)
def get_layout(db: Session = Depends(get_db)):
    saved = db.get(AppSetting, LAYOUT_KEY)
    return Layout.model_validate_json(saved.value) if saved else DEFAULT_LAYOUT


@router.put("/layout", response_model=Layout)
def save_layout(layout: Layout, db: Session = Depends(get_db)):
    value = json.dumps(layout.model_dump(mode="json", exclude_none=True))
    saved = db.get(AppSetting, LAYOUT_KEY)
    if saved:
        saved.value = value
    else:
        db.add(AppSetting(key=LAYOUT_KEY, value=value))
    db.commit()
    return layout


@router.delete("/layout", response_model=Layout)
def reset_layout(db: Session = Depends(get_db)):
    """Go back to the default dashboard."""
    saved = db.get(AppSetting, LAYOUT_KEY)
    if saved:
        db.delete(saved)
        db.commit()
    return DEFAULT_LAYOUT
