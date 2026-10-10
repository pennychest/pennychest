"""Suggesting a category for each card tap with the model chosen for the "taps" task.

This runs after the tap has been saved and the phone has had its reply, so a slow or failing
model never delays or breaks a tap."""

import time

from sqlalchemy import select
from sqlalchemy.orm import Session

from pennychest.accounts.models import Account, AccountType
from pennychest.ai import learned
from pennychest.ai.categorise import TxnInput, categorise_transactions
from pennychest.ai.models import AIRequestLog
from pennychest.ai.providers import ProviderError, resolve_task
from pennychest.core.database import SessionLocal
from pennychest.core.lookup_models import CategorisationSource
from pennychest.rules.engine import _get_match_type_map, find_matching_rule, get_all_rules_sorted
from pennychest.taps.models import CardTap
from pennychest.transactions.models import Posting, Transaction

UNCATEGORISED_PATH = "Expenses:Uncategorised"


def _rule_matches(db: Session, tap: CardTap) -> bool:
    rules = get_all_rules_sorted(db)
    return bool(rules) and find_matching_rule(tap.merchant, rules, _get_match_type_map(db))


def categorise_tap(db: Session, tap: CardTap) -> None:
    """Ask the model for the tap's spending category and remember it on the tap. Skipped if
    one of the user's rules already covers the merchant. Raises ProviderError if the model
    can't be reached (the call is logged either way)."""
    if tap.ai_account_id is not None or _rule_matches(db, tap):
        return
    if learned.suggest(db, [tap.merchant])[0] is not None:
        return  # the user's own history already knows this one
    provider, cfg, model = resolve_task(db, "taps")
    categories = {
        a.full_path: a.id
        for a in db.execute(select(Account).where(Account.type == AccountType.EXPENSE)).scalars()
        if ":" in a.full_path and a.full_path != UNCATEGORISED_PATH
    }
    if not categories:
        return
    description = f"{tap.merchant} ({tap.currency} {abs(tap.amount):.2f}"
    description += ", refund)" if tap.amount < 0 else ")"
    started = time.monotonic()
    request = response = error = None
    try:
        results, request, response = categorise_transactions(
            provider, cfg, model, [TxnInput(tap.id, description)], list(categories)
        )
    except ProviderError as e:
        error = str(e)
        raise
    finally:
        db.add(
            AIRequestLog(
                provider=f"{provider.id}:{model}",
                operation="tap_categorise",
                request_data=request,
                response_data=response,
                duration_ms=int((time.monotonic() - started) * 1000),
                error=error,
            )
        )
        db.commit()
    if results:
        tap.ai_account_id = categories[results[0].account_full_path]
        tap.ai_confidence = results[0].confidence
        apply_tap_category(db, tap)
        db.commit()


def apply_tap_category(db: Session, tap: CardTap) -> bool:
    """Give the statement transaction a tap is matched to the tap's AI category, but only if
    the import left it uncategorised. Returns whether anything changed."""
    if tap.transaction_id is None or tap.ai_account_id is None:
        return False
    txn = db.get(Transaction, tap.transaction_id)
    uncategorised = [
        p
        for p in (txn.postings if txn else [])
        if p.account.full_path == UNCATEGORISED_PATH and p.categorised_by.name == "import_default"
    ]
    if len(uncategorised) != 1:
        return False
    ai_source = db.execute(
        select(CategorisationSource).where(CategorisationSource.name == "ai")
    ).scalar_one()
    posting: Posting = uncategorised[0]
    posting.account_id = tap.ai_account_id
    posting.categorised_by_id = ai_source.id
    posting.rule_id = None
    return True


def categorise_tap_in_background(tap_id: int) -> None:
    """Run after the ingest response has been sent, in its own session. Failures are already
    in the AI request logs, so they're not raised."""
    with SessionLocal() as db:
        tap = db.get(CardTap, tap_id)
        if tap is None:
            return
        try:
            categorise_tap(db, tap)
        except ProviderError:
            pass
