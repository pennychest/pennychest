"""Parsing of wallet tap payloads and reconciliation against imported transactions."""

import re
from datetime import date, datetime, timedelta
from decimal import Decimal, InvalidOperation

from sqlalchemy import select
from sqlalchemy.orm import Session

from pennychest.accounts.models import Account
from pennychest.ai.config import task_available
from pennychest.ai.matching import MIN_PROBABILITY, ask_pairs
from pennychest.ai.providers import ProviderError
from pennychest.core.database import SessionLocal
from pennychest.taps.ai import apply_tap_category
from pennychest.taps.models import CardTap, WalletCard
from pennychest.transactions.models import Posting, Transaction

# Statements usually show a payment on the tap date or a few days after it
# (the posting date). Allow a day earlier for timezone/midnight edge cases.
MATCH_DAYS_BEFORE = 1
MATCH_DAYS_AFTER = 7

_CURRENCY_SYMBOLS = {"£": "GBP", "€": "EUR", "$": "USD"}

_DATE_FORMATS = [
    "%d %b %Y",
    "%d %B %Y",
    "%d/%m/%Y",
    "%d/%m/%y",
    "%Y/%m/%d",
    "%b %d, %Y",
    "%B %d, %Y",
]


def parse_amount(value: object) -> tuple[Decimal, str | None]:
    """Parse an amount such as "£12.50", "12,50 €", "-3.00" or 12.5.

    Returns the amount and the currency code if one could be detected.
    """
    if isinstance(value, (int, float, Decimal)):
        return Decimal(str(value)), None
    if not isinstance(value, str) or not value.strip():
        raise ValueError("amount is required")

    text = value.strip()
    currency = next((code for sym, code in _CURRENCY_SYMBOLS.items() if sym in text), None)
    if currency is None:
        code = re.search(r"\b([A-Z]{3})\b", text)
        currency = code.group(1) if code else None

    number = re.sub(r"[^0-9.,\-]", "", text)
    if "," in number and "." in number:
        # Whichever separator comes last is the decimal point
        if number.rfind(",") > number.rfind("."):
            number = number.replace(".", "").replace(",", ".")
        else:
            number = number.replace(",", "")
    elif "," in number:
        # "12,50" is a decimal comma; "1,250" is a thousands separator
        head, _, tail = number.rpartition(",")
        number = f"{head.replace(',', '')}.{tail}" if len(tail) == 2 else number.replace(",", "")

    try:
        return Decimal(number), currency
    except InvalidOperation:
        raise ValueError(f"could not parse amount {value!r}")


def parse_tap_date(value: object) -> date:
    """Parse the tap date, defaulting to today when none is sent."""
    if value is None or (isinstance(value, str) and not value.strip()):
        return date.today()
    if not isinstance(value, str):
        raise ValueError(f"could not parse date {value!r}")

    text = value.strip()
    try:
        return datetime.fromisoformat(text.replace("Z", "+00:00")).date()
    except ValueError:
        pass
    # Drop any time part, e.g. "26 Sep 2026 at 17:02"
    date_part = re.split(r"\s+at\s+|,\s*\d{1,2}:\d{2}|\s+\d{1,2}:\d{2}", text)[0].strip()
    for fmt in _DATE_FORMATS:
        try:
            return datetime.strptime(date_part, fmt).date()
        except ValueError:
            continue
    raise ValueError(f"could not parse date {value!r}")


def card_accounts(db: Session) -> dict[str, Account]:
    """Map of paired card name -> account."""
    cards = db.execute(select(WalletCard)).scalars().all()
    return {c.card_name: c.account for c in cards}


def _words(text: str) -> set[str]:
    return {w for w in re.findall(r"[a-z0-9]+", text.lower()) if len(w) >= 3}


TAP_QUESTION = (
    "Are {a} (a card payment the user's phone recorded at the till) and {b} (a line on their "
    "card statement) the same payment? Statements often show a payment processor prefix such "
    "as SQ * or SUMUP *, a reference number, a location, or the company's legal name instead "
    "of the shop's name."
)


def _ask_model(db: Session, tap: CardTap, candidates: list[Transaction]) -> Transaction | None:
    """The candidate the matching model is confident is this tap, if any."""
    tap_side = {
        "merchant": tap.merchant,
        "date": tap.tapped_on.isoformat(),
        "amount": str(tap.amount),
    }
    statement_lines = [
        {"description": t.description, "date": t.date.isoformat()} for t in candidates
    ]
    probabilities = ask_pairs(
        db, "tap_match", TAP_QUESTION, [(tap_side, line) for line in statement_lines]
    )
    best, probability = max(zip(candidates, probabilities), key=lambda pair: pair[1])
    return best if probability >= MIN_PROBABILITY else None


def reconcile(db: Session, *, use_model: bool = True) -> int:
    """Link unmatched taps to imported transactions. Returns the number linked.

    A tap matches a transaction on the tap's paired account whose account-side
    posting is the negated tap amount, dated within the match window. When
    several transactions qualify, prefer the one whose description shares a word
    with the merchant. If that doesn't settle it and a matching model is chosen,
    the model picks, and the tap is left unmatched unless it's confident; with
    `use_model` off such taps are left for a later reconcile that can ask it.
    Otherwise the transaction closest to the tap date wins.
    """
    accounts = card_accounts(db)
    taps = (
        db.execute(
            select(CardTap)
            .where(
                CardTap.transaction_id.is_(None),
                CardTap.dismissed.is_(False),
                CardTap.card_name.in_(list(accounts)),
            )
            .order_by(CardTap.tapped_on, CardTap.id)
        )
        .scalars()
        .all()
    )
    if not taps:
        return 0

    claimed = set(
        db.execute(select(CardTap.transaction_id).where(CardTap.transaction_id.is_not(None)))
        .scalars()
        .all()
    )

    linked = 0
    model_ready = task_available(db, "matching")
    for tap in taps:
        account = accounts[tap.card_name]
        if tap.currency != account.currency:
            continue
        candidates = (
            db.execute(
                select(Transaction)
                .join(Posting, Posting.transaction_id == Transaction.id)
                .where(
                    Posting.account_id == account.id,
                    Posting.amount == -tap.amount,
                    Transaction.date >= tap.tapped_on - timedelta(days=MATCH_DAYS_BEFORE),
                    Transaction.date <= tap.tapped_on + timedelta(days=MATCH_DAYS_AFTER),
                )
            )
            .scalars()
            .all()
        )
        candidates = [t for t in candidates if t.id not in claimed]
        if not candidates:
            continue

        merchant_words = _words(tap.merchant)
        sharing = [t for t in candidates if merchant_words & _words(t.description)]
        best = None
        if len(sharing) == 1:
            best = sharing[0]
        elif len(candidates) > 1 and model_ready:
            if not use_model:
                continue
            try:
                best = _ask_model(db, tap, sharing or candidates)
            except ProviderError:
                model_ready = False  # logged; fall back to dates for the rest
            else:
                if best is None:
                    continue
        if best is None:
            best = min(
                sharing or candidates,
                key=lambda t: (abs((t.date - tap.tapped_on).days), t.id),
            )
        tap.transaction_id = best.id
        claimed.add(best.id)
        # Imports leave unknown merchants uncategorised; the tap's AI category fills the gap.
        apply_tap_category(db, tap)
        linked += 1

    db.flush()
    return linked


def reconcile_in_background() -> None:
    """Run after a tap's ingest response has been sent, in its own session, so taps that need
    the matching model are linked without the phone waiting on it."""
    with SessionLocal() as db:
        reconcile(db)
        db.commit()
