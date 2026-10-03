import re
from decimal import Decimal
from typing import Literal

from pydantic import Field
from sqlalchemy import select
from sqlalchemy.orm import Session, joinedload

from pennychest.accounts.models import Account, AccountType
from pennychest.actions.common import (
    CATEGORY_TYPES,
    UNCATEGORISED_PATH,
    find_category,
    money,
    path_segment,
)
from pennychest.actions.registry import ActionError, ActionInput, Scope, action
from pennychest.budgets.models import Budget
from pennychest.core.lookup_models import BudgetPeriod, CategorisationSource, MatchType
from pennychest.rules.models import Rule
from pennychest.transactions.models import Posting, Transaction


def _path_taken(db: Session, path: str) -> bool:
    return db.execute(select(Account.id).where(Account.full_path == path)).first() is not None


def move_account(db: Session, account: Account, new_path: str, new_name: str) -> int:
    """Give an account a new path and name, carrying its subcategories along. Returns how
    many subcategories moved."""
    old_path = account.full_path
    descendants = (
        db.execute(
            select(Account).where(Account.full_path.startswith(f"{old_path}:", autoescape=True))
        )
        .scalars()
        .all()
    )
    for child in descendants:
        child.full_path = new_path + child.full_path[len(old_path) :]
    account.full_path = new_path
    account.name = new_name
    return len(descendants)


class CreateCategoryInput(ActionInput):
    name: str = Field(min_length=1, max_length=60, description='Display name, e.g. "Amazon Prime".')
    parent: str = Field(
        description='The category to create it under (path or name), e.g. "Expenses:Entertainment" '
        'or "Income".'
    )


@action(
    """
    Create a new spending or income category under an existing one. It takes the parent's
    type, so a category under "Expenses" is for spending.
    """,
    scope=Scope.ORGANISE,
)
def create_category(db: Session, params: CreateCategoryInput) -> dict:
    parent = find_category(db, params.parent)
    segment = path_segment(params.name)
    if not segment:
        raise ActionError("The name needs at least one letter or number.")
    path = f"{parent.full_path}:{segment}"
    if _path_taken(db, path):
        raise ActionError(f"A category {path!r} already exists.")
    account = Account(
        name=params.name.strip(),
        full_path=path,
        parent_id=parent.id,
        type=parent.type,
        currency=parent.currency,
    )
    db.add(account)
    db.commit()
    return {
        "created": {"path": account.full_path, "name": account.name, "type": account.type.value},
        "_change": {
            "summary": f'Created category "{account.name}" under {parent.full_path}',
            "undo": {"account_id": account.id},
        },
    }


class RenameCategoryInput(ActionInput):
    category: str = Field(description="The category to rename (path or name).")
    new_name: str = Field(min_length=1, max_length=60, description="Its new display name.")


@action(
    """
    Rename a category. Its subcategories, budgets, rules and transactions stay attached.
    """,
    scope=Scope.ORGANISE,
)
def rename_category(db: Session, params: RenameCategoryInput) -> dict:
    account = find_category(db, params.category)
    if ":" not in account.full_path or account.full_path == UNCATEGORISED_PATH:
        raise ActionError(f"{account.full_path!r} can't be renamed; the app relies on it.")
    segment = path_segment(params.new_name)
    if not segment:
        raise ActionError("The name needs at least one letter or number.")
    old_path, old_name = account.full_path, account.name
    new_path = f"{old_path.rsplit(':', 1)[0]}:{segment}"
    if new_path != old_path and _path_taken(db, new_path):
        raise ActionError(f"A category {new_path!r} already exists.")
    moved = move_account(db, account, new_path, params.new_name.strip())
    db.commit()
    return {
        "renamed": {"from": old_path, "to": new_path, "subcategories_moved": moved},
        "_change": {
            "summary": f'Renamed category "{old_name}" to "{account.name}"',
            "undo": {
                "account_id": account.id,
                "from_path": old_path,
                "from_name": old_name,
                "to_path": new_path,
            },
        },
    }


class CreateRuleInput(ActionInput):
    pattern: str = Field(min_length=1, max_length=200, description="Text to match in descriptions.")
    category: str = Field(description="Category to assign (path or name).")
    match_type: Literal["substring", "prefix", "regex"] = Field(
        default="substring",
        description='"substring" matches anywhere, "prefix" only at the start, "regex" is a '
        "case-insensitive regular expression.",
    )
    priority: int = Field(default=10, ge=1, le=100, description="Higher-priority rules win.")
    description: str | None = Field(
        default=None, max_length=200, description="A short note on what the rule matches."
    )


@action(
    """
    Create a rule that automatically categorises future imported transactions whose
    description matches the pattern. It doesn't change existing transactions; use
    recategorise_transactions for those.
    """,
    scope=Scope.ORGANISE,
)
def create_rule(db: Session, params: CreateRuleInput) -> dict:
    category = find_category(db, params.category)
    if params.match_type == "regex":
        try:
            re.compile(params.pattern)
        except re.error as e:
            raise ActionError(f"That regular expression is invalid: {e}")
    match_type_id = db.execute(
        select(MatchType.id).where(MatchType.name == params.match_type)
    ).scalar_one()
    duplicate = db.execute(
        select(Rule.id).where(
            Rule.pattern == params.pattern,
            Rule.match_type_id == match_type_id,
            Rule.target_account_id == category.id,
        )
    ).first()
    if duplicate:
        raise ActionError("That rule already exists.")
    rule = Rule(
        pattern=params.pattern,
        match_type_id=match_type_id,
        target_account_id=category.id,
        priority=params.priority,
        source="ai",
        description=params.description,
    )
    db.add(rule)
    db.commit()
    return {
        "created": {
            "id": rule.id,
            "pattern": rule.pattern,
            "match_type": params.match_type,
            "category": category.full_path,
            "priority": rule.priority,
        },
        "_change": {
            "summary": f'Created rule "{rule.pattern}" → {category.full_path}',
            "undo": {"rule_id": rule.id},
        },
    }


class RecategoriseInput(ActionInput):
    transaction_ids: list[int] = Field(
        min_length=1, max_length=200, description="IDs from list_transactions."
    )
    category: str = Field(description="The category to move them to (path or name).")


@action(
    """
    Move transactions to a different category. Transactions split across several categories,
    or with no category (such as transfers between the user's own accounts), are skipped and
    reported.
    """,
    scope=Scope.TRANSACTIONS,
)
def recategorise_transactions(db: Session, params: RecategoriseInput) -> dict:
    category = find_category(db, params.category)
    source_id = db.execute(
        select(CategorisationSource.id).where(CategorisationSource.name == "ai")
    ).scalar_one()
    ids = list(dict.fromkeys(params.transaction_ids))
    txns = {
        t.id: t
        for t in db.execute(
            select(Transaction)
            .where(Transaction.id.in_(ids))
            .options(joinedload(Transaction.postings).joinedload(Posting.account))
        )
        .unique()
        .scalars()
    }
    updated, skipped, before = [], [], []
    for txn_id in ids:
        txn = txns.get(txn_id)
        if not txn:
            skipped.append({"id": txn_id, "reason": "not found"})
            continue
        postings = [p for p in txn.postings if p.account.type in CATEGORY_TYPES]
        if not postings:
            skipped.append({"id": txn_id, "reason": "has no category (e.g. a transfer)"})
            continue
        if len(postings) > 1:
            skipped.append({"id": txn_id, "reason": "split across several categories"})
            continue
        posting = postings[0]
        before.append(
            {
                "id": posting.id,
                "account_id": posting.account_id,
                "categorised_by_id": posting.categorised_by_id,
                "rule_id": posting.rule_id,
            }
        )
        posting.account_id = category.id
        posting.categorised_by_id = source_id
        posting.rule_id = None
        updated.append(txn_id)
    db.commit()
    result = {"category": category.full_path, "updated": updated, "skipped": skipped}
    if updated:
        count = f"{len(updated)} transaction{'s' if len(updated) != 1 else ''}"
        result["_change"] = {
            "summary": f"Moved {count} to {category.full_path}",
            "undo": {"postings": before},
        }
    return result


class SetBudgetInput(ActionInput):
    category: str = Field(description="Spending category (path or name); includes subcategories.")
    amount: Decimal = Field(gt=0, description="The spending limit for the period.")
    period: Literal["monthly", "annual"] = "monthly"


def _period_id(db: Session, name: str) -> int:
    return db.execute(select(BudgetPeriod.id).where(BudgetPeriod.name == name)).scalar_one()


@action(
    """
    Create a budget for a spending category, or change the limit if it already has one for
    that period. A category can have both a monthly and an annual budget.
    """,
    scope=Scope.ORGANISE,
)
def set_budget(db: Session, params: SetBudgetInput) -> dict:
    category = find_category(db, params.category)
    if category.type != AccountType.EXPENSE:
        raise ActionError("Budgets can only be set on spending categories.")
    period_id = _period_id(db, params.period)
    budget = db.execute(
        select(Budget).where(Budget.account_id == category.id, Budget.period_id == period_id)
    ).scalar_one_or_none()
    previous = money(budget.amount) if budget else None
    if budget:
        budget.amount = params.amount
    else:
        db.add(Budget(account_id=category.id, period_id=period_id, amount=params.amount))
    db.commit()
    verb = (
        f"Changed the {params.period} budget for"
        if previous
        else f"Set a {params.period} budget for"
    )
    return {
        "budget": {
            "category": category.full_path,
            "period": params.period,
            "limit": money(params.amount),
        },
        "previous_limit": previous,
        "_change": {
            "summary": f"{verb} {category.full_path} to £{money(params.amount)}",
            "undo": {"account_id": category.id, "period_id": period_id, "previous": previous},
        },
    }


class DeleteBudgetInput(ActionInput):
    category: str = Field(description="Spending category (path or name).")
    period: Literal["monthly", "annual"] = "monthly"


@action("Remove a category's monthly or annual budget.", scope=Scope.ORGANISE)
def delete_budget(db: Session, params: DeleteBudgetInput) -> dict:
    category = find_category(db, params.category)
    budget = db.execute(
        select(Budget).where(
            Budget.account_id == category.id,
            Budget.period_id == _period_id(db, params.period),
        )
    ).scalar_one_or_none()
    if not budget:
        raise ActionError(f"{category.full_path!r} has no {params.period} budget.")
    removed = money(budget.amount)
    period_id = budget.period_id
    db.delete(budget)
    db.commit()
    return {
        "deleted": {"category": category.full_path, "period": params.period, "limit": removed},
        "_change": {
            "summary": f"Removed the {params.period} budget for {category.full_path}",
            "undo": {"account_id": category.id, "period_id": period_id, "amount": removed},
        },
    }
