"""Higher-level read actions that answer broad questions ("where can I save?", "what's
changed?") in one call, so a model doesn't have to piece them together from many small
lookups."""

import csv
import io
import re
from collections import Counter, defaultdict
from datetime import date, timedelta
from decimal import Decimal
from statistics import median
from typing import Literal

from pydantic import Field
from sqlalchemy import select
from sqlalchemy.orm import Session, joinedload

from pennychest.accounts.models import Account, AccountType
from pennychest.actions.common import CATEGORY_TYPES, MONEY_ACCOUNT_TYPES, money
from pennychest.actions.read import group_totals
from pennychest.actions.registry import ActionError, ActionInput, action
from pennychest.ai.models import MerchantLabel
from pennychest.budgets.models import Budget
from pennychest.budgets.service import category_spend, period_bounds
from pennychest.transactions.models import Posting, Transaction

# Cadence name -> (typical days between charges, shortest gap, longest gap, charges a year).
CADENCES = {
    "weekly": (7, 5, 9, Decimal(52)),
    "monthly": (30, 25, 35, Decimal(12)),
    "quarterly": (91, 80, 100, Decimal(4)),
    "yearly": (365, 340, 390, Decimal(1)),
}


def _months_before(month_start: date, months: int) -> date:
    """The first of the month `months` before a month's first day."""
    index = month_start.year * 12 + month_start.month - 1 - months
    return date(index // 12, index % 12 + 1, 1)


def merchant_key(description: str) -> str:
    """A description with references stripped, so repeat charges from one merchant group
    together: "NETFLIX.COM 8829301" and "NETFLIX.COM 1123987" both become "netflix com"."""
    words = re.split(r"[\s*/#]+", description.lower())
    kept = (re.sub(r"[^a-z&]+", " ", w) for w in words if not re.search(r"\d", w))
    return " ".join(" ".join(kept).split()) or description.lower().strip()


def find_recurring(db: Session, end: date, lookback_days: int = 400) -> list[dict]:
    """Charges that repeat on a regular cadence, most expensive per year first."""
    rows = db.execute(
        select(
            Transaction.id,
            Transaction.date,
            Transaction.description,
            Account.full_path,
            Posting.amount,
        )
        .join(Posting, Posting.transaction_id == Transaction.id)
        .join(Account, Posting.account_id == Account.id)
        .where(
            Account.type == AccountType.EXPENSE,
            Transaction.date > end - timedelta(days=lookback_days),
            Transaction.date <= end,
        )
    )
    charges: dict[int, dict] = {}
    for txn_id, txn_date, description, path, amount in rows:
        charge = charges.setdefault(
            txn_id,
            {"date": txn_date, "description": description, "amount": Decimal(0), "paths": []},
        )
        charge["amount"] += Decimal(str(amount))
        charge["paths"].append(path)

    by_merchant: dict[str, list[dict]] = defaultdict(list)
    for charge in charges.values():
        if charge["amount"] > 0:  # refunds aren't charges
            by_merchant[merchant_key(charge["description"])].append(charge)

    found = []
    for key, group in by_merchant.items():
        if len(group) < 2:
            continue
        group.sort(key=lambda c: c["date"])
        gaps = [(b["date"] - a["date"]).days for a, b in zip(group, group[1:])]
        typical_gap = median(gaps)
        for name, (days, shortest, longest, per_year) in CADENCES.items():
            if shortest <= typical_gap <= longest:
                break
        else:
            continue
        regular = sum(shortest <= g <= longest for g in gaps)
        if (name != "yearly" and len(group) < 3) or regular < len(gaps) * 2 / 3:
            continue
        last = group[-1]
        if (end - last["date"]).days > days * 1.5:
            continue  # stopped
        recent = [c["amount"] for c in group[-3:]]
        amount = median(recent)
        earlier = [c["amount"] for c in group[:-1][-3:]]
        found.append(
            {
                "merchant": last["description"],
                "merchant_key": key,
                "category": Counter(p for c in group for p in c["paths"]).most_common(1)[0][0],
                "cadence": name,
                "amount": money(amount),
                "amount_varies": max(recent) > min(recent) * Decimal("1.15"),
                "yearly_cost": money(amount * per_year),
                "charges": len(group),
                "first_date": group[0]["date"].isoformat(),
                "last_date": last["date"].isoformat(),
                "last_amount": money(last["amount"]),
                # What it usually cost before the latest charge, to spot bills going up
                "usual_amount": money(median(earlier)),
            }
        )
    found.sort(key=lambda r: Decimal(r["yearly_cost"]), reverse=True)
    return found


def spending_charges(db: Session, start: date, end: date) -> dict[int, dict]:
    """Spending per transaction between two dates: its date, description, total and category
    (the largest expense posting). Refunds are left out."""
    rows = db.execute(
        select(
            Transaction.id, Transaction.date, Transaction.description, Account.full_path,
            Posting.amount,
        )
        .join(Posting, Posting.transaction_id == Transaction.id)
        .join(Account, Posting.account_id == Account.id)
        .where(
            Account.type == AccountType.EXPENSE,
            Transaction.date >= start,
            Transaction.date <= end,
        )
    )
    charges: dict[int, dict] = {}
    for txn_id, txn_date, description, path, amount in rows:
        amount = Decimal(str(amount))
        charge = charges.setdefault(
            txn_id,
            {"id": txn_id, "date": txn_date, "description": description, "amount": Decimal(0),
             "category": path, "largest": amount},
        )
        charge["amount"] += amount
        if amount > charge["largest"]:
            charge["category"], charge["largest"] = path, amount
    return {i: c for i, c in charges.items() if c["amount"] > 0}


class RecurringPaymentsInput(ActionInput):
    on: date | None = Field(
        default=None, description="Report payments still active on this date. Defaults to today."
    )
    kind: Literal["any", "subscription", "bill", "other"] = Field(
        default="any",
        description="Only payments of this kind: subscription (services signed up for), bill "
        "(household essentials and contracts) or other (ordinary spending that happens to "
        "repeat, such as groceries).",
    )
    limit: int = Field(default=30, ge=1, le=100)


@action(
    """
    Subscriptions, bills and other charges that repeat weekly, monthly, quarterly or yearly
    and are still active, found from the last 13 months of spending. Each has its typical
    amount, whether that amount varies, and what it costs a year; most expensive per year
    first. Use this for questions about subscriptions, bills or regular outgoings. Each has a
    kind (subscription, bill or other) once the insights model has labelled it, otherwise
    "unlabelled"; pass kind to list only one kind.
    """
)
def recurring_payments(db: Session, params: RecurringPaymentsInput) -> dict:
    kinds = dict(db.execute(select(MerchantLabel.merchant_key, MerchantLabel.kind)).all())
    found = find_recurring(db, params.on or date.today())
    for r in found:
        r["kind"] = kinds.get(r["merchant_key"], "unlabelled")
    if params.kind != "any":
        found = [r for r in found if r["kind"] == params.kind]
    shown = found[: params.limit]
    return {
        "payments": shown,
        "yearly_total": money(sum((Decimal(r["yearly_cost"]) for r in found), Decimal(0))),
        "payments_not_shown": len(found) - len(shown),
    }


# Unusual-charge scores from 0 (normal) to 1; at or above this a charge is reported.
UNUSUAL_SCORE = 0.5
# A recurring payment's latest charge must beat its usual amount by this much to count as a rise.
RISE_FRACTION = Decimal("0.02")


class UnusualChargesInput(ActionInput):
    start_date: date = Field(description="First day to include.")
    end_date: date | None = Field(
        default=None, description="Last day to include. Defaults to today."
    )
    limit: int = Field(default=20, ge=1, le=100)


@action(
    """
    Charges that look unusual for their merchant or category (much more than usual, a large
    one-off, or a possible mistake or fraud), and subscriptions or bills whose latest charge
    went up, between two dates. Use this for "any unusual charges?", "anything odd on my card?"
    or "did any bills go up?". Charges are only judged once an insights model is set up in
    Settings > AI; unscored_charges says how many in the period haven't been judged.
    """
)
def unusual_charges(db: Session, params: UnusualChargesInput) -> dict:
    end = params.end_date or date.today()
    if end < params.start_date:
        raise ActionError("end_date must not be before start_date.")
    charges = spending_charges(db, params.start_date, end)
    scores = dict(
        db.execute(
            select(Transaction.id, Transaction.unusual_score).where(
                Transaction.id.in_(list(charges))
            )
        ).all()
    )
    unusual = sorted(
        (
            {
                "transaction_id": c["id"],
                "date": c["date"].isoformat(),
                "description": c["description"],
                "amount": money(c["amount"]),
                "category": c["category"],
                "unusual_score": scores[c["id"]],
            }
            for c in charges.values()
            if (scores.get(c["id"]) or 0) >= UNUSUAL_SCORE
        ),
        key=lambda c: (-c["unusual_score"], c["date"]),
    )

    kinds = dict(db.execute(select(MerchantLabel.merchant_key, MerchantLabel.kind)).all())
    rises = []
    for r in find_recurring(db, end):
        kind = kinds.get(r["merchant_key"], "unlabelled")
        last, usual = Decimal(r["last_amount"]), Decimal(r["usual_amount"])
        if (
            kind != "other"
            and date.fromisoformat(r["last_date"]) >= params.start_date
            and last > usual * (1 + RISE_FRACTION)
        ):
            rises.append(
                {
                    "merchant": r["merchant"],
                    "kind": kind,
                    "cadence": r["cadence"],
                    "usual_amount": r["usual_amount"],
                    "latest_amount": r["last_amount"],
                    "latest_date": r["last_date"],
                    "increase": money(last - usual),
                }
            )
    rises.sort(key=lambda r: Decimal(r["increase"]), reverse=True)
    return {
        "unusual_charges": unusual[: params.limit],
        "unusual_not_shown": max(len(unusual) - params.limit, 0),
        "price_rises": rises,
        "unscored_charges": sum(1 for i in charges if scores.get(i) is None),
    }


class ComparePeriodsInput(ActionInput):
    start_date: date = Field(description="First day of the period to look at.")
    end_date: date = Field(description="Last day of the period to look at (inclusive).")
    compare_start_date: date | None = Field(
        default=None,
        description="First day of the period to compare against. Defaults to the same number "
        "of days immediately before start_date.",
    )
    compare_end_date: date | None = Field(
        default=None, description="Last day of the period to compare against (inclusive)."
    )
    group_by: Literal["category", "merchant"] = Field(default="category")
    category: str | None = Field(
        default=None,
        description="Only include this category and its subcategories (path or name).",
    )
    category_depth: int | None = Field(
        default=None,
        ge=1,
        le=6,
        description="When grouping by category, roll subcategories up to this many path levels.",
    )
    limit: int = Field(default=15, ge=1, le=100, description="Maximum number of groups.")


@action(
    """
    Compare spending in two periods side by side, grouped by category or merchant, with the
    change in each group; groups that changed most come first. Use this instead of calling
    spending_summary once per period.
    """
)
def compare_periods(db: Session, params: ComparePeriodsInput) -> dict:
    if params.end_date < params.start_date:
        raise ActionError("end_date must be on or after start_date.")
    if (params.compare_start_date is None) != (params.compare_end_date is None):
        raise ActionError("Give both compare_start_date and compare_end_date, or neither.")
    if params.compare_start_date is None:
        length = params.end_date - params.start_date
        compare_end = params.start_date - timedelta(days=1)
        compare_start = compare_end - length
    else:
        compare_start, compare_end = params.compare_start_date, params.compare_end_date
        if compare_end < compare_start:
            raise ActionError("compare_end_date must be on or after compare_start_date.")

    def totals(start: date, end: date):
        return group_totals(
            db,
            start,
            end,
            group_by=params.group_by,
            category=params.category,
            category_depth=params.category_depth,
        )

    now, _, now_total, now_unreviewed = totals(params.start_date, params.end_date)
    before, _, before_total, before_unreviewed = totals(compare_start, compare_end)
    keys = sorted(
        set(now) | set(before), key=lambda k: abs(now.get(k, 0) - before.get(k, 0)), reverse=True
    )
    shown = keys[: params.limit]
    zero = Decimal(0)
    return {
        "period": {
            "start_date": params.start_date.isoformat(),
            "end_date": params.end_date.isoformat(),
            "total": money(now_total),
            "unreviewed": money(now_unreviewed),
        },
        "compared_with": {
            "start_date": compare_start.isoformat(),
            "end_date": compare_end.isoformat(),
            "total": money(before_total),
            "unreviewed": money(before_unreviewed),
        },
        "change": money(now_total - before_total),
        "groups": [
            {
                "key": k,
                "total": money(now.get(k, zero)),
                "compared_total": money(before.get(k, zero)),
                "change": money(now.get(k, zero) - before.get(k, zero)),
            }
            for k in shown
        ],
        "groups_not_shown": len(keys) - len(shown),
    }


class SavingsOpportunitiesInput(ActionInput):
    months: int = Field(
        default=3,
        ge=1,
        le=12,
        description="How many recent full calendar months to average over.",
    )
    on: date | None = Field(default=None, description="Treat this as today. Defaults to today.")


@action(
    """
    One-call overview of where the user could save money: average monthly spending by
    category and merchant over recent full months, categories that went up last month
    compared with the months before, active subscriptions and bills with their yearly cost,
    and budgets that are overspent this period. Start here for broad questions like "how can
    I save money?", "where does my money go?" or "what should I cut back on?", then drill in
    with other actions only if needed.
    """
)
def savings_opportunities(db: Session, params: SavingsOpportunitiesInput) -> dict:
    today = params.on or date.today()
    this_month = today.replace(day=1)
    start = _months_before(this_month, params.months)
    end = this_month - timedelta(days=1)
    months = Decimal(params.months)

    by_category, _, total, unreviewed = group_totals(db, start, end, category_depth=2)
    by_merchant, merchant_txns, _, _ = group_totals(db, start, end, group_by="merchant")

    # Last full month against the average of the months before it.
    last_month = _months_before(this_month, 1)
    baseline_start = _months_before(last_month, params.months)
    recent, _, _, _ = group_totals(db, last_month, end, category_depth=2)
    baseline, _, _, _ = group_totals(
        db, baseline_start, last_month - timedelta(days=1), category_depth=2
    )
    rising = []
    for key, amount in recent.items():
        usual = baseline.get(key, Decimal(0)) / months
        if amount - usual >= 1:
            rising.append((key, amount, usual))
    rising.sort(key=lambda r: r[1] - r[2], reverse=True)

    recurring = find_recurring(db, today)

    overspent = []
    budgets = (
        db.execute(select(Budget).options(joinedload(Budget.account), joinedload(Budget.period)))
        .scalars()
        .all()
    )
    for b in budgets:
        period_start, period_end = period_bounds(b.period.name, today)
        spent, _ = category_spend(db, b.account, period_start, period_end)
        if spent > b.amount:
            overspent.append(
                {
                    "category": b.account.full_path,
                    "period": b.period.name,
                    "limit": money(b.amount),
                    "spent": money(spent),
                    "over_by": money(spent - b.amount),
                }
            )

    def top(totals: dict[str, Decimal], n: int) -> list[tuple[str, Decimal]]:
        return sorted(totals.items(), key=lambda kv: kv[1], reverse=True)[:n]

    return {
        "averaged_over": {
            "start_date": start.isoformat(),
            "end_date": end.isoformat(),
            "months": params.months,
        },
        "monthly_average_spending": money(total / months),
        "unreviewed": money(unreviewed),
        "top_categories": [
            {"category": k, "monthly_average": money(v / months)} for k, v in top(by_category, 10)
        ],
        "top_merchants": [
            {
                "merchant": k,
                "monthly_average": money(v / months),
                "transactions": len(merchant_txns[k]),
            }
            for k, v in top(by_merchant, 10)
        ],
        "rising_last_month": {
            "month": last_month.isoformat()[:7],
            "categories": [
                {
                    "category": k,
                    "last_month": money(amount),
                    "usual_month": money(usual),
                    "increase": money(amount - usual),
                }
                for k, amount, usual in rising[:5]
            ],
        },
        "recurring_payments": recurring[:15],
        "recurring_yearly_total": money(
            sum((Decimal(r["yearly_cost"]) for r in recurring), Decimal(0))
        ),
        "overspent_budgets": overspent,
    }


# Above this many transactions the digest lists merchants instead, to keep it a sensible size
# for a model's context (a transaction row is roughly 25 tokens).
DIGEST_MAX_TRANSACTIONS = 1000


def _csv(header: list[str], rows: list[list]) -> str:
    out = io.StringIO()
    writer = csv.writer(out, lineterminator="\n")
    writer.writerow(header)
    writer.writerows(rows)
    return out.getvalue()


class TransactionsDigestInput(ActionInput):
    start_date: date = Field(description="First day to include.")
    end_date: date = Field(description="Last day to include (inclusive).")
    detail: Literal["auto", "transactions", "merchants"] = Field(
        default="auto",
        description='"transactions" lists every transaction, "merchants" one row per merchant. '
        f'"auto" lists transactions when there are at most {DIGEST_MAX_TRANSACTIONS}, '
        "otherwise merchants.",
    )


@action(
    f"""
    Every categorised transaction in a date range as compact CSV (transfers between the
    user's own accounts are left out), so you can read them all at once and spot patterns:
    subscriptions, habits, unusual or growing spending. Amounts are positive for spending and
    negative for income or refunds; status "pending" means unreviewed. Over
    {DIGEST_MAX_TRANSACTIONS} transactions it gives one row per merchant instead (count,
    total, typical amount, first and last date). Use it to find things; for totals quote
    "total_spending", "total_income" or the other actions rather than adding up rows yourself.
    """
)
def transactions_digest(db: Session, params: TransactionsDigestInput) -> dict:
    if params.end_date < params.start_date:
        raise ActionError("end_date must be on or after start_date.")
    txns = (
        db.execute(
            select(Transaction)
            .options(joinedload(Transaction.postings).joinedload(Posting.account))
            .where(Transaction.date >= params.start_date, Transaction.date <= params.end_date)
            .order_by(Transaction.date, Transaction.id)
        )
        .unique()
        .scalars()
        .all()
    )
    spending = income = unreviewed = Decimal(0)
    rows = []
    for t in txns:
        categories = [p for p in t.postings if p.account.type in CATEGORY_TYPES]
        if not categories:
            continue
        paid_with = [p for p in t.postings if p.account.type in MONEY_ACCOUNT_TYPES]
        for p in categories:
            amount = Decimal(str(p.amount))
            if p.account.type == AccountType.EXPENSE:
                spending += amount
                if t.status == "pending":
                    unreviewed += amount
            else:
                income -= amount
        rows.append(
            {
                "date": t.date,
                "description": t.description,
                "amount": sum((Decimal(str(p.amount)) for p in categories), Decimal(0)),
                "category": ";".join(p.account.full_path for p in categories),
                "account": ";".join(p.account.full_path for p in paid_with),
                "status": t.status,
            }
        )

    detail = params.detail
    if detail == "auto":
        detail = "transactions" if len(rows) <= DIGEST_MAX_TRANSACTIONS else "merchants"
    if detail == "transactions":
        header = ["date", "description", "amount", "category", "account", "status"]
        table = [
            [
                r["date"].isoformat(),
                r["description"],
                money(r["amount"]),
                r["category"],
                r["account"],
                r["status"],
            ]
            for r in rows
        ]
    else:
        by_merchant: dict[str, list[dict]] = defaultdict(list)
        for r in rows:
            by_merchant[merchant_key(r["description"])].append(r)
        groups = sorted(
            by_merchant.values(), key=lambda g: sum(r["amount"] for r in g), reverse=True
        )
        header = [
            "merchant",
            "category",
            "transactions",
            "total",
            "typical_amount",
            "first_date",
            "last_date",
        ]
        table = [
            [
                g[-1]["description"],
                Counter(r["category"] for r in g).most_common(1)[0][0],
                len(g),
                money(sum(r["amount"] for r in g)),
                money(median(r["amount"] for r in g)),
                g[0]["date"].isoformat(),
                g[-1]["date"].isoformat(),
            ]
            for g in groups
        ]
    return {
        "start_date": params.start_date.isoformat(),
        "end_date": params.end_date.isoformat(),
        "detail": detail,
        "transactions": len(rows),
        "total_spending": money(spending),
        "total_income": money(income),
        "unreviewed": money(unreviewed),
        "csv": _csv(header, table),
    }
