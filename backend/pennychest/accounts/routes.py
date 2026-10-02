from datetime import date
from decimal import Decimal, InvalidOperation

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel
from sqlalchemy import and_, select
from sqlalchemy.orm import Session, aliased

from pennychest.accounts.models import Account, AccountType
from pennychest.accounts.schemas import AccountCreate, AccountResponse, AccountUpdate
from pennychest.core.database import get_db
from pennychest.core.lookup_models import CategorisationSource
from pennychest.transactions.models import Posting, Transaction

router = APIRouter(prefix="/api/accounts", tags=["accounts"])


@router.get("", response_model=list[AccountResponse])
def list_accounts(db: Session = Depends(get_db)):
    result = db.execute(select(Account).order_by(Account.full_path))
    return result.scalars().all()


@router.get("/{account_id}", response_model=AccountResponse)
def get_account(account_id: int, db: Session = Depends(get_db)):
    account = db.get(Account, account_id)
    if not account:
        raise HTTPException(status_code=404, detail="Account not found")
    return account


@router.post("", response_model=AccountResponse, status_code=201)
def create_account(payload: AccountCreate, db: Session = Depends(get_db)):
    existing = db.execute(
        select(Account).where(Account.full_path == payload.full_path)
    ).scalar_one_or_none()
    if existing:
        raise HTTPException(status_code=409, detail="Account with this path already exists")

    if payload.parent_id:
        parent = db.get(Account, payload.parent_id)
        if not parent:
            raise HTTPException(status_code=400, detail="Parent account not found")

    account = Account(**payload.model_dump())
    db.add(account)
    db.commit()
    db.refresh(account)
    return account


@router.put("/{account_id}", response_model=AccountResponse)
def update_account(account_id: int, payload: AccountUpdate, db: Session = Depends(get_db)):
    account = db.get(Account, account_id)
    if not account:
        raise HTTPException(status_code=404, detail="Account not found")

    update_data = payload.model_dump(exclude_unset=True)
    for key, value in update_data.items():
        setattr(account, key, value)

    db.commit()
    db.refresh(account)
    return account


@router.delete("/{account_id}", status_code=204)
def delete_account(account_id: int, db: Session = Depends(get_db)):
    account = db.get(Account, account_id)
    if not account:
        raise HTTPException(status_code=404, detail="Account not found")

    children = db.execute(
        select(Account).where(Account.parent_id == account_id)
    ).scalars().all()
    if children:
        raise HTTPException(status_code=400, detail="Cannot delete account with children")

    db.delete(account)
    db.commit()


_OPENING_BALANCES_PATH = "Equity:OpeningBalances"
_OPENING_BALANCE_DESC = "Opening Balance"


class OpeningBalanceResponse(BaseModel):
    amount: str
    date: date
    transaction_id: int


class OpeningBalanceUpdate(BaseModel):
    # Signed amount on the bank-account side of the posting:
    #   - positive for asset accounts in funds
    #   - negative for liability accounts (credit cards) with a balance owed
    # The Equity:OpeningBalances posting is created with the inverse amount.
    amount: Decimal
    date: date


def _get_opening_balances_account(db: Session) -> Account:
    account = db.execute(
        select(Account).where(Account.full_path == _OPENING_BALANCES_PATH)
    ).scalar_one_or_none()
    if account is None:
        raise HTTPException(
            status_code=500,
            detail=(
                f"Required account '{_OPENING_BALANCES_PATH}' is missing. "
                "Run the chart-of-accounts seed and try again."
            ),
        )
    return account


def _find_opening_balance_transaction(
    db: Session, account_id: int, equity_account_id: int
) -> tuple[Transaction, Posting] | None:
    """Locate the opening-balance transaction for an account, if one exists.

    A transaction qualifies if it has exactly two postings, one against the
    given bank account and one against Equity:OpeningBalances. There should
    be at most one such transaction per account; if multiple exist we pick
    the earliest (by date).
    """
    bank_post = aliased(Posting)
    equity_post = aliased(Posting)
    txn = (
        db.execute(
            select(Transaction, bank_post)
            .join(bank_post, bank_post.transaction_id == Transaction.id)
            .join(equity_post, equity_post.transaction_id == Transaction.id)
            .where(
                and_(
                    bank_post.account_id == account_id,
                    equity_post.account_id == equity_account_id,
                )
            )
            .order_by(Transaction.date.asc(), Transaction.id.asc())
            .limit(1)
        ).first()
    )
    if txn is None:
        return None
    return txn[0], txn[1]


@router.get("/{account_id}/opening-balance")
def get_account_opening_balance(account_id: int, db: Session = Depends(get_db)):
    """Return the opening-balance posting for an account, or null if none.

    The opening balance is modelled as a Transaction with two postings: the
    bank/CC account on one side and Equity:OpeningBalances on the other. The
    bank-side amount is what the user thinks of as "the opening balance".
    """
    account = db.get(Account, account_id)
    if not account:
        raise HTTPException(status_code=404, detail="Account not found")

    equity = _get_opening_balances_account(db)
    found = _find_opening_balance_transaction(db, account_id, equity.id)
    if found is None:
        return None
    transaction, bank_posting = found
    return OpeningBalanceResponse(
        amount=str(bank_posting.amount),
        date=transaction.date,
        transaction_id=transaction.id,
    )


@router.put("/{account_id}/opening-balance")
def set_account_opening_balance(
    account_id: int,
    payload: OpeningBalanceUpdate,
    db: Session = Depends(get_db),
):
    """Create or update the opening-balance transaction for an account.

    If a transaction already exists, its date and posting amounts are updated
    in place. Otherwise a new confirmed transaction is created. Postings are
    tagged with the "manual" categorisation source — the user explicitly
    accepts this value when they apply it from the import flow.
    """
    account = db.get(Account, account_id)
    if not account:
        raise HTTPException(status_code=404, detail="Account not found")
    if account.type not in (AccountType.ASSET, AccountType.LIABILITY):
        raise HTTPException(
            status_code=400,
            detail="Opening balance is only meaningful for asset or liability accounts",
        )

    try:
        amount = Decimal(payload.amount)
    except (InvalidOperation, TypeError):
        raise HTTPException(status_code=400, detail="Invalid amount")

    equity = _get_opening_balances_account(db)
    manual_source = db.execute(
        select(CategorisationSource).where(CategorisationSource.name == "manual")
    ).scalar_one()

    found = _find_opening_balance_transaction(db, account_id, equity.id)
    if found is not None:
        txn, bank_posting = found
        txn.date = payload.date
        bank_posting.amount = amount
        # Update the equity-side posting to mirror the new amount.
        for p in txn.postings:
            if p.id != bank_posting.id and p.account_id == equity.id:
                p.amount = -amount
                break
        db.commit()
        db.refresh(txn)
        return OpeningBalanceResponse(
            amount=str(bank_posting.amount),
            date=txn.date,
            transaction_id=txn.id,
        )

    txn = Transaction(
        date=payload.date,
        description=_OPENING_BALANCE_DESC,
        status="confirmed",
        currency=account.currency,
    )
    db.add(txn)
    db.flush()

    bank_posting = Posting(
        transaction_id=txn.id,
        account_id=account_id,
        amount=amount,
        categorised_by_id=manual_source.id,
    )
    equity_posting = Posting(
        transaction_id=txn.id,
        account_id=equity.id,
        amount=-amount,
        categorised_by_id=manual_source.id,
    )
    db.add(bank_posting)
    db.add(equity_posting)
    db.commit()
    db.refresh(txn)
    return OpeningBalanceResponse(
        amount=str(amount),
        date=txn.date,
        transaction_id=txn.id,
    )


@router.get("/tree/hierarchy", response_model=list[AccountResponse])
def get_account_tree(
    account_type: str | None = None, db: Session = Depends(get_db)
):
    query = select(Account).order_by(Account.full_path)
    if account_type:
        query = query.where(Account.type == account_type)
    result = db.execute(query)
    return result.scalars().all()


@router.get("/export")
def export_accounts(db: Session = Depends(get_db)):
    """Export chart of accounts as JSON."""
    accounts = db.execute(
        select(Account).order_by(Account.full_path)
    ).scalars().all()

    # Build parent path lookup
    id_to_path = {a.id: a.full_path for a in accounts}

    exported = []
    for account in accounts:
        item = {
            "name": account.name,
            "full_path": account.full_path,
            "type": account.type.value if isinstance(account.type, AccountType) else account.type,
        }
        if account.parent_id and account.parent_id in id_to_path:
            item["parent_path"] = id_to_path[account.parent_id]
        if account.currency != "GBP":
            item["currency"] = account.currency
        exported.append(item)

    return exported


@router.post("/import")
def import_accounts(accounts_data: list[dict], db: Session = Depends(get_db)):
    """Import chart of accounts from JSON.

    Only works on an empty database (no existing accounts).
    """
    existing_count = db.execute(select(Account).limit(1)).scalar_one_or_none()
    if existing_count:
        raise HTTPException(
            status_code=400,
            detail="Cannot import accounts when accounts already exist. "
            "Use this on a fresh database only.",
        )

    path_to_id: dict[str, int] = {}
    created = 0

    for acc_data in accounts_data:
        parent_id = None
        parent_path = acc_data.get("parent_path")
        if parent_path and parent_path in path_to_id:
            parent_id = path_to_id[parent_path]

        account = Account(
            name=acc_data["name"],
            full_path=acc_data["full_path"],
            parent_id=parent_id,
            type=acc_data["type"],
            currency=acc_data.get("currency", "GBP"),
        )
        db.add(account)
        db.flush()
        path_to_id[account.full_path] = account.id
        created += 1

    db.commit()
    return {"accounts_created": created}
