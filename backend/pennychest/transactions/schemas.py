from __future__ import annotations

from datetime import date as DateType
from datetime import datetime
from decimal import Decimal
from typing import Optional

from pydantic import BaseModel


class PostingCreate(BaseModel):
    account_id: int
    amount: Decimal
    categorised_by_id: int
    rule_id: Optional[int] = None


class PostingResponse(BaseModel):
    id: int
    transaction_id: int
    account_id: int
    amount: Decimal
    categorised_by_id: int
    rule_id: Optional[int] = None
    account_full_path: Optional[str] = None

    model_config = {"from_attributes": True}


class TransactionCreate(BaseModel):
    date: DateType
    description: str
    status: str = "pending"
    postings: list[PostingCreate]


class TransactionUpdate(BaseModel):
    date: Optional[DateType] = None
    description: Optional[str] = None
    status: Optional[str] = None


class TransactionResponse(BaseModel):
    id: int
    date: DateType
    description: str
    status: str
    currency: str = "GBP"
    import_batch_id: Optional[int] = None
    created_at: datetime
    postings: list[PostingResponse] = []

    model_config = {"from_attributes": True}


class TransactionListParams(BaseModel):
    status: Optional[str] = None
    account_id: Optional[int] = None
    date_from: Optional[DateType] = None
    date_to: Optional[DateType] = None
    search: Optional[str] = None
    page: int = 1
    per_page: int = 50
