import hmac
from typing import Literal

from fastapi import APIRouter, BackgroundTasks, Depends, Header, HTTPException
from pydantic import BaseModel
from sqlalchemy import func, select
from sqlalchemy.orm import Session

from pennychest.accounts.models import Account
from pennychest.ai import learned
from pennychest.ai.config import runs_automatically
from pennychest.ai.providers import ProviderError, resolve_task
from pennychest.core.database import get_db
from pennychest.rules.engine import _get_match_type_map, find_matching_rule, get_all_rules_sorted
from pennychest.taps.ai import categorise_tap, categorise_tap_in_background
from pennychest.taps.models import CardTap, WalletCard
from pennychest.taps.schemas import (
    TapIngest,
    TapResponse,
    WalletCardPair,
    WalletCardResponse,
)
from pennychest.taps.service import (
    card_accounts,
    parse_amount,
    parse_tap_date,
    reconcile,
    reconcile_in_background,
)
from pennychest.taps.token import clear_tap_token, get_tap_token, regenerate_tap_token

router = APIRouter(prefix="/api/taps", tags=["taps"])


def require_api_token(
    authorization: str = Header(default=""), db: Session = Depends(get_db)
) -> None:
    expected = get_tap_token(db)
    if not expected:
        raise HTTPException(
            status_code=503, detail="Tap ingest is off: create a token in Settings > Card taps"
        )
    token = authorization[7:] if authorization.startswith("Bearer ") else ""
    if not hmac.compare_digest(token, expected):
        raise HTTPException(status_code=401, detail="Invalid API token")


class TapTokenResponse(BaseModel):
    token: str | None


# Declared before the /{tap_id} routes so "token" isn't parsed as a tap id.
@router.get("/token", response_model=TapTokenResponse)
def read_tap_token(db: Session = Depends(get_db)):
    return TapTokenResponse(token=get_tap_token(db))


@router.post("/token", response_model=TapTokenResponse)
def create_tap_token(db: Session = Depends(get_db)):
    """Create a new token, replacing any existing one."""
    return TapTokenResponse(token=regenerate_tap_token(db))


@router.delete("/token", status_code=204)
def delete_tap_token(db: Session = Depends(get_db)):
    clear_tap_token(db)


def _status(tap: CardTap) -> str:
    if tap.dismissed:
        return "dismissed"
    return "reconciled" if tap.transaction_id is not None else "unmatched"


def _to_responses(db: Session, taps: list[CardTap]) -> list[TapResponse]:
    accounts = card_accounts(db)
    rules = get_all_rules_sorted(db)
    match_type_map = _get_match_type_map(db)
    rule_targets: dict[int, str] = {}
    if rules:
        rule_targets = {
            a.id: a.full_path
            for a in db.execute(
                select(Account).where(Account.id.in_({r.target_account_id for r in rules}))
            ).scalars()
        }

    # What the user's own history says, for taps no rule covers
    unmatched = [t for t in taps if not find_matching_rule(t.merchant, rules, match_type_map)]
    learned_paths = {}
    for tap, suggestion in zip(unmatched, learned.suggest(db, [t.merchant for t in unmatched])):
        if suggestion is not None:
            account = db.get(Account, suggestion[0])
            if account is not None:
                learned_paths[tap.id] = (account.full_path, suggestion[1])

    responses = []
    for tap in taps:
        account = accounts.get(tap.card_name) if tap.card_name else None
        match = find_matching_rule(tap.merchant, rules, match_type_map)
        # The user's own rules win, then what they've taught it, then the AI's suggestion
        if match:
            suggestion = rule_targets.get(match.target_account_id)
            source, confidence = "rule", None
        elif tap.id in learned_paths:
            suggestion, confidence = learned_paths[tap.id]
            source = "learned"
        elif tap.ai_account is not None:
            suggestion, source, confidence = tap.ai_account.full_path, "ai", tap.ai_confidence
        else:
            suggestion = source = confidence = None
        txn = tap.transaction
        responses.append(
            TapResponse(
                id=tap.id,
                tapped_on=tap.tapped_on,
                merchant=tap.merchant,
                amount=tap.amount,
                currency=tap.currency,
                card_name=tap.card_name,
                status=_status(tap),
                account_id=account.id if account else None,
                account_full_path=account.full_path if account else None,
                suggested_account_full_path=suggestion,
                suggestion_source=source if suggestion else None,
                suggestion_confidence=confidence,
                transaction_id=tap.transaction_id,
                transaction_description=txn.description if txn else None,
                transaction_date=txn.date if txn else None,
                created_at=tap.created_at,
            )
        )
    return responses


@router.post(
    "", response_model=TapResponse, status_code=201, dependencies=[Depends(require_api_token)]
)
def ingest_tap(body: TapIngest, background: BackgroundTasks, db: Session = Depends(get_db)):
    """Record a card payment sent by the phone wallet shortcut."""
    merchant = (body.merchant or "").strip()
    if not merchant:
        raise HTTPException(status_code=400, detail="merchant is required")
    try:
        amount, detected_currency = parse_amount(body.amount)
        tapped_on = parse_tap_date(body.date)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))
    if amount == 0:
        raise HTTPException(status_code=400, detail="amount must not be zero")

    currency = (body.currency or detected_currency or "GBP").strip().upper()
    tap = CardTap(
        tapped_on=tapped_on,
        merchant=merchant,
        amount=amount,
        currency=currency,
        card_name=(body.card or "").strip() or None,
    )
    db.add(tap)
    db.flush()
    # The statement may already have been imported if the tap is sent late
    reconcile(db, use_model=False)
    db.commit()
    db.refresh(tap)
    # After the response is sent, so the phone never waits on (or sees) a model.
    if runs_automatically(db, "taps"):
        background.add_task(categorise_tap_in_background, tap.id)
    if tap.transaction_id is None and runs_automatically(db, "matching"):
        background.add_task(reconcile_in_background)
    return _to_responses(db, [tap])[0]


@router.get("", response_model=list[TapResponse])
def list_taps(
    status: Literal["unmatched", "reconciled", "dismissed", "all"] = "unmatched",
    db: Session = Depends(get_db),
):
    query = select(CardTap).order_by(CardTap.tapped_on.desc(), CardTap.id.desc())
    if status == "unmatched":
        query = query.where(CardTap.dismissed.is_(False), CardTap.transaction_id.is_(None))
    elif status == "reconciled":
        query = query.where(CardTap.dismissed.is_(False), CardTap.transaction_id.is_not(None))
    elif status == "dismissed":
        query = query.where(CardTap.dismissed.is_(True))
    return _to_responses(db, list(db.execute(query).scalars().all()))


def _get_tap(db: Session, tap_id: int) -> CardTap:
    tap = db.get(CardTap, tap_id)
    if not tap:
        raise HTTPException(status_code=404, detail="Tap not found")
    return tap


@router.post("/{tap_id}/categorise", response_model=TapResponse)
def categorise_tap_now(tap_id: int, db: Session = Depends(get_db)):
    """Suggest a category for a tap, for when card taps are categorised on demand rather than
    as each one arrives."""
    tap = _get_tap(db, tap_id)
    try:
        resolve_task(db, "taps")
        categorise_tap(db, tap)
    except ProviderError as e:
        raise HTTPException(status_code=400, detail=str(e))
    db.refresh(tap)
    return _to_responses(db, [tap])[0]


@router.post("/{tap_id}/dismiss", response_model=TapResponse)
def dismiss_tap(tap_id: int, db: Session = Depends(get_db)):
    tap = _get_tap(db, tap_id)
    tap.dismissed = True
    tap.transaction_id = None
    db.commit()
    db.refresh(tap)
    return _to_responses(db, [tap])[0]


@router.post("/{tap_id}/restore", response_model=TapResponse)
def restore_tap(tap_id: int, db: Session = Depends(get_db)):
    tap = _get_tap(db, tap_id)
    tap.dismissed = False
    db.flush()
    reconcile(db)
    db.commit()
    db.refresh(tap)
    return _to_responses(db, [tap])[0]


@router.delete("/{tap_id}", status_code=204)
def delete_tap(tap_id: int, db: Session = Depends(get_db)):
    db.delete(_get_tap(db, tap_id))
    db.commit()


@router.post("/reconcile")
def reconcile_taps(db: Session = Depends(get_db)):
    linked = reconcile(db)
    db.commit()
    return {"reconciled": linked}


@router.get("/cards", response_model=list[WalletCardResponse])
def list_cards(db: Session = Depends(get_db)):
    """Every card name seen in a tap or paired, with its pairing."""
    counts = dict(
        db.execute(
            select(CardTap.card_name, func.count())
            .where(CardTap.card_name.is_not(None))
            .group_by(CardTap.card_name)
        ).all()
    )
    accounts = card_accounts(db)
    return [
        WalletCardResponse(
            card_name=name,
            account_id=accounts[name].id if name in accounts else None,
            account_full_path=accounts[name].full_path if name in accounts else None,
            tap_count=counts.get(name, 0),
        )
        for name in sorted(set(counts) | set(accounts), key=str.lower)
    ]


@router.put("/cards", response_model=WalletCardResponse)
def pair_card(body: WalletCardPair, db: Session = Depends(get_db)):
    """Pair a wallet card with an asset or liability account, or unpair it."""
    card_name = body.card_name.strip()
    if not card_name:
        raise HTTPException(status_code=400, detail="card_name is required")
    card = db.execute(
        select(WalletCard).where(WalletCard.card_name == card_name)
    ).scalar_one_or_none()

    account = None
    if body.account_id is None:
        if card:
            db.delete(card)
    else:
        account = db.get(Account, body.account_id)
        if not account:
            raise HTTPException(status_code=400, detail="Account not found")
        if account.type not in ("asset", "liability"):
            raise HTTPException(
                status_code=400, detail="Cards can only be paired with asset or liability accounts"
            )
        if card:
            card.account_id = account.id
        else:
            db.add(WalletCard(card_name=card_name, account_id=account.id))
    db.flush()

    # Links made under the old pairing may no longer be valid
    for tap in db.execute(
        select(CardTap).where(CardTap.card_name == card_name, CardTap.transaction_id.is_not(None))
    ).scalars():
        tap.transaction_id = None
    db.flush()
    reconcile(db)
    db.commit()

    tap_count = db.execute(
        select(func.count()).select_from(CardTap).where(CardTap.card_name == card_name)
    ).scalar_one()
    return WalletCardResponse(
        card_name=card_name,
        account_id=account.id if account else None,
        account_full_path=account.full_path if account else None,
        tap_count=tap_count,
    )
