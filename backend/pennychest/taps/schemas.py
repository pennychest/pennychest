from __future__ import annotations

from datetime import date as DateType
from datetime import datetime
from decimal import Decimal
from typing import Literal

from pydantic import BaseModel

TapStatus = Literal["unmatched", "reconciled", "dismissed"]


class TapIngest(BaseModel):
    """Payload sent by the phone shortcut. Fields are loosely typed because
    the shortcut sends whatever the wallet trigger provides."""

    merchant: str | None = None
    amount: str | float | int | None = None
    date: str | None = None
    card: str | None = None
    currency: str | None = None


class TapResponse(BaseModel):
    id: int
    tapped_on: DateType
    merchant: str
    amount: Decimal
    currency: str
    card_name: str | None
    status: TapStatus
    account_id: int | None
    account_full_path: str | None
    suggested_account_full_path: str | None
    # "rule", "learned" (from the user's own history) or "ai"; in that order of preference
    suggestion_source: str | None = None
    suggestion_confidence: float | None = None
    transaction_id: int | None
    transaction_description: str | None
    transaction_date: DateType | None
    created_at: datetime


class WalletCardResponse(BaseModel):
    card_name: str
    account_id: int | None
    account_full_path: str | None
    tap_count: int


class WalletCardPair(BaseModel):
    card_name: str
    # None removes the pairing
    account_id: int | None
