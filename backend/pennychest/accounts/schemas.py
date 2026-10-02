from pydantic import BaseModel

from pennychest.accounts.models import AccountType


class AccountCreate(BaseModel):
    name: str
    full_path: str
    parent_id: int | None = None
    type: AccountType
    currency: str = "GBP"
    bank_identifier: str | None = None
    icon: str | None = None


class AccountUpdate(BaseModel):
    name: str | None = None
    full_path: str | None = None
    parent_id: int | None = None
    type: AccountType | None = None
    currency: str | None = None
    bank_identifier: str | None = None
    icon: str | None = None


class AccountResponse(BaseModel):
    id: int
    name: str
    full_path: str
    parent_id: int | None
    type: AccountType
    currency: str
    bank_identifier: str | None = None
    icon: str | None = None

    model_config = {"from_attributes": True}
