from collections import defaultdict
from datetime import date, timedelta
from decimal import Decimal
from typing import Literal

from pydantic import Field
from sqlalchemy import func, select
from sqlalchemy.orm import Session, joinedload

from pennychest.accounts.models import Account, AccountType
from pennychest.actions.common import (
    CATEGORY_TYPES,
    MONEY_ACCOUNT_TYPES,
    describe_transaction,
    find_account,
    find_category,
    in_account,
    money,
)
from pennychest.actions.registry import ActionError, ActionInput, action
from pennychest.budgets.models import Budget
from pennychest.budgets.service import category_spend, period_bounds
from pennychest.core.lookup_models import MatchType
from pennychest.rules.models import Rule
from pennychest.taps.models import CardTap
from pennychest.transactions.models import Posting, Transaction


class ListCategoriesInput(ActionInput):
    kind: Literal["expense", "income", "all"] = Field(
        default="all", description="Spending categories, income categories, or both."
    )


@action(
    """
    List the user's categories (spending and income), as colon-separated paths from general
    to specific, e.g. "Expenses:Food:Groceries". Spending in a category includes its
    subcategories.
    """
)
def list_categories(db: Session, params: ListCategoriesInput) -> dict:
    types = {
        "expense": (AccountType.EXPENSE,),
        "income": (AccountType.INCOME,),
        "all": CATEGORY_TYPES,
    }[params.kind]
    accounts = (
        db.execute(select(Account).where(Account.type.in_(types)).order_by(Account.full_path))
        .scalars()
        .all()
    )
    return {
        "categories": [
            {"path": a.full_path, "name": a.name, "type": a.type.value} for a in accounts
        ]
    }


class ListAccountsInput(ActionInput):
    pass


@action("List the user's bank accounts, cards and other asset or liability accounts.")
def list_accounts(db: Session, params: ListAccountsInput) -> dict:
    accounts = (
        db.execute(
            select(Account).where(Account.type.in_(MONEY_ACCOUNT_TYPES)).order_by(Account.full_path)
        )
        .scalars()
        .all()
    )
    return {
        "accounts": [
            {"path": a.full_path, "name": a.name, "type": a.type.value, "currency": a.currency}
            for a in accounts
        ]
    }


class SpendingSummaryInput(ActionInput):
    start_date: date = Field(description="First day to include.")
    end_date: date = Field(description="Last day to include (inclusive).")
    group_by: Literal["category", "month", "merchant"] = Field(
        default="category",
        description="Group totals by category, by calendar month, or by merchant "
        "(the transaction description).",
    )
    kind: Literal["expenses", "income"] = Field(
        default="expenses", description="Summarise spending or income."
    )
    category: str | None = Field(
        default=None,
        description="Only include this category and its subcategories (path or name).",
    )
    category_depth: int | None = Field(
        default=None,
        ge=1,
        le=6,
        description="When grouping by category, roll subcategories up to this many path "
        'levels, e.g. 2 groups "Expenses:Food:Groceries" under "Expenses:Food".',
    )
    limit: int = Field(default=25, ge=1, le=200, description="Maximum number of groups.")


@action(
    """
    Total spending (or income) over a date range, grouped by category, month or merchant.
    Amounts are exact sums from the database; spending is positive and refunds reduce it.
    "unreviewed" is the part from imported transactions the user hasn't reviewed yet. Groups
    are sorted largest first (months are sorted by date).
    """
)
def spending_summary(db: Session, params: SpendingSummaryInput) -> dict:
    if params.end_date < params.start_date:
        raise ActionError("end_date must be on or after start_date.")
    totals, transactions, total, unreviewed = group_totals(
        db,
        params.start_date,
        params.end_date,
        kind=params.kind,
        group_by=params.group_by,
        category=params.category,
        category_depth=params.category_depth,
    )
    if params.group_by == "month":
        keys = sorted(totals)
    else:
        keys = sorted(totals, key=lambda k: totals[k], reverse=True)
    shown = keys[: params.limit]
    return {
        "kind": params.kind,
        "start_date": params.start_date.isoformat(),
        "end_date": params.end_date.isoformat(),
        "total": money(total),
        "unreviewed": money(unreviewed),
        "groups": [
            {"key": k, "total": money(totals[k]), "transactions": len(transactions[k])}
            for k in shown
        ],
        "groups_not_shown": len(keys) - len(shown),
    }


def group_totals(
    db: Session,
    start: date,
    end: date,
    *,
    kind: str = "expenses",
    group_by: str = "category",
    category: str | None = None,
    category_depth: int | None = None,
) -> tuple[dict[str, Decimal], dict[str, set[int]], Decimal, Decimal]:
    """Spending (or income) from start to end inclusive: the total and transaction ids for
    each group, the overall total, and the part of it that is unreviewed."""
    account_type = AccountType.EXPENSE if kind == "expenses" else AccountType.INCOME
    sign = Decimal(1) if kind == "expenses" else Decimal(-1)
    query = (
        select(
            Transaction.id,
            Transaction.date,
            Transaction.description,
            Transaction.status,
            Account.full_path,
            Posting.amount,
        )
        .join(Posting, Posting.transaction_id == Transaction.id)
        .join(Account, Posting.account_id == Account.id)
        .where(
            Account.type == account_type,
            Transaction.date >= start,
            Transaction.date <= end,
        )
    )
    if category:
        query = query.where(in_account(find_category(db, category)))

    totals: dict[str, Decimal] = defaultdict(Decimal)
    transactions: dict[str, set[int]] = defaultdict(set)
    total = unreviewed = Decimal(0)
    for txn_id, txn_date, description, status, path, amount in db.execute(query):
        value = sign * Decimal(str(amount))
        total += value
        if status == "pending":
            unreviewed += value
        if group_by == "month":
            key = txn_date.isoformat()[:7]
        elif group_by == "merchant":
            key = description
        elif category_depth:
            key = ":".join(path.split(":")[:category_depth])
        else:
            key = path
        totals[key] += value
        transactions[key].add(txn_id)
    return totals, transactions, total, unreviewed


class ListTransactionsInput(ActionInput):
    start_date: date | None = Field(default=None, description="Earliest date to include.")
    end_date: date | None = Field(default=None, description="Latest date to include.")
    search: str | None = Field(
        default=None, description="Case-insensitive text to look for in the description."
    )
    category: str | None = Field(
        default=None, description="Only this category and its subcategories (path or name)."
    )
    exact_category: bool = Field(
        default=False,
        description="With category, leave out its subcategories: only transactions recorded "
        "against the category itself.",
    )
    account: str | None = Field(
        default=None, description="Only transactions on this bank account or card (path or name)."
    )
    status: Literal["pending", "confirmed", "any"] = Field(
        default="any", description='"pending" means imported but not yet reviewed.'
    )
    min_amount: Decimal | None = Field(
        default=None, description="Smallest categorised amount to include."
    )
    max_amount: Decimal | None = Field(
        default=None, description="Largest categorised amount to include."
    )
    limit: int = Field(default=50, ge=1, le=200)
    offset: int = Field(default=0, ge=0)


def _postings_in(account: Account):
    return (
        select(Posting.transaction_id)
        .join(Account, Posting.account_id == Account.id)
        .where(in_account(account))
    )


@action(
    """
    Find transactions, newest first. Each has its category (or categories), the amount
    categorised (positive for spending, negative for income or refunds), the bank account or
    card it was paid from or into, and whether it has been reviewed. Returns the total number
    of matches so results can be paged with offset.
    """
)
def list_transactions(db: Session, params: ListTransactionsInput) -> dict:
    query = select(Transaction)
    if params.start_date:
        query = query.where(Transaction.date >= params.start_date)
    if params.end_date:
        query = query.where(Transaction.date <= params.end_date)
    if params.search:
        query = query.where(Transaction.description.icontains(params.search, autoescape=True))
    if params.status != "any":
        query = query.where(Transaction.status == params.status)
    if params.category:
        category = find_category(db, params.category)
        if params.exact_category:
            query = query.where(
                Transaction.id.in_(
                    select(Posting.transaction_id).where(Posting.account_id == category.id)
                )
            )
        else:
            query = query.where(Transaction.id.in_(_postings_in(category)))
    if params.account:
        money_account = find_account(db, params.account, MONEY_ACCOUNT_TYPES, what="account")
        query = query.where(Transaction.id.in_(_postings_in(money_account)))
    if params.min_amount is not None or params.max_amount is not None:
        amounts = (
            select(Posting.transaction_id)
            .join(Account, Posting.account_id == Account.id)
            .where(Account.type.in_(CATEGORY_TYPES))
        )
        if params.min_amount is not None:
            amounts = amounts.where(Posting.amount >= params.min_amount)
        if params.max_amount is not None:
            amounts = amounts.where(Posting.amount <= params.max_amount)
        query = query.where(Transaction.id.in_(amounts))

    total = db.execute(select(func.count()).select_from(query.subquery())).scalar_one()
    txns = (
        db.execute(
            query.options(joinedload(Transaction.postings).joinedload(Posting.account))
            .order_by(Transaction.date.desc(), Transaction.id.desc())
            .limit(params.limit)
            .offset(params.offset)
        )
        .unique()
        .scalars()
        .all()
    )

    results = [describe_transaction(t) for t in txns]
    return {"total": total, "offset": params.offset, "transactions": results}


class ListRulesInput(ActionInput):
    pass


@action(
    """
    List the rules that categorise imported transactions automatically: a pattern matched
    against the description, the category it assigns, and a priority (higher wins).
    """
)
def list_rules(db: Session, params: ListRulesInput) -> dict:
    match_types = {m.id: m.name for m in db.execute(select(MatchType)).scalars()}
    rules = db.execute(
        select(Rule, Account.full_path)
        .join(Account, Rule.target_account_id == Account.id)
        .order_by(Rule.priority.desc(), Rule.pattern)
    ).all()
    return {
        "rules": [
            {
                "id": rule.id,
                "pattern": rule.pattern,
                "match_type": match_types.get(rule.match_type_id),
                "category": path,
                "priority": rule.priority,
                "description": rule.description,
            }
            for rule, path in rules
        ]
    }


class GetBudgetsInput(ActionInput):
    on: date | None = Field(
        default=None,
        description="Any date in the month (or year, for annual budgets) to report on. "
        "Defaults to today.",
    )


@action(
    """
    The user's budgets with how much has been spent against each in the period containing
    the given date. "remaining" is negative when a budget is overspent.
    """
)
def get_budgets(db: Session, params: GetBudgetsInput) -> dict:
    on = params.on or date.today()
    budgets = (
        db.execute(select(Budget).options(joinedload(Budget.account), joinedload(Budget.period)))
        .scalars()
        .all()
    )
    results = []
    for b in budgets:
        start, end = period_bounds(b.period.name, on)
        spent, unreviewed = category_spend(db, b.account, start, end)
        results.append(
            {
                "category": b.account.full_path,
                "period": b.period.name,
                "period_start": start.isoformat(),
                "period_end": (end - timedelta(days=1)).isoformat(),
                "limit": money(b.amount),
                "spent": money(spent),
                "unreviewed": money(unreviewed),
                "remaining": money(b.amount - spent),
            }
        )
    results.sort(key=lambda r: (r["period"], r["category"]))
    return {"budgets": results}


class ListCardTapsInput(ActionInput):
    status: Literal["unmatched", "reconciled", "dismissed", "all"] = Field(
        default="unmatched",
        description='"unmatched" taps are card payments not yet seen on an imported statement.',
    )
    limit: int = Field(default=50, ge=1, le=200)


@action(
    """
    Card payments recorded by the phone wallet shortcut. Unmatched taps are spending that
    hasn't appeared on an imported statement yet.
    """
)
def list_card_taps(db: Session, params: ListCardTapsInput) -> dict:
    query = select(CardTap)
    if params.status == "unmatched":
        query = query.where(CardTap.transaction_id.is_(None), CardTap.dismissed.is_(False))
    elif params.status == "reconciled":
        query = query.where(CardTap.transaction_id.is_not(None))
    elif params.status == "dismissed":
        query = query.where(CardTap.dismissed.is_(True))
    taps = (
        db.execute(query.order_by(CardTap.tapped_on.desc(), CardTap.id.desc()).limit(params.limit))
        .scalars()
        .all()
    )
    return {
        "taps": [
            {
                "id": t.id,
                "date": t.tapped_on.isoformat(),
                "merchant": t.merchant,
                "amount": money(t.amount),
                "currency": t.currency,
                "card": t.card_name,
                "matched_transaction_id": t.transaction_id,
                "dismissed": t.dismissed,
            }
            for t in taps
        ]
    }
