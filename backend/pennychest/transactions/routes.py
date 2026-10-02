from datetime import date

from fastapi import APIRouter, Depends, HTTPException, Query
from sqlalchemy import func, select
from sqlalchemy.orm import Session, joinedload

from pennychest.accounts.models import Account
from pennychest.actions.common import in_account
from pennychest.core.database import get_db
from pennychest.transactions.models import Posting, Transaction
from pennychest.transactions.service import remove_transaction
from pennychest.transactions.schemas import (
    PostingResponse,
    TransactionCreate,
    TransactionResponse,
    TransactionUpdate,
)

router = APIRouter(prefix="/api/transactions", tags=["transactions"])


def _build_posting_response(posting: Posting) -> PostingResponse:
    return PostingResponse(
        id=posting.id,
        transaction_id=posting.transaction_id,
        account_id=posting.account_id,
        amount=posting.amount,
        categorised_by_id=posting.categorised_by_id,
        rule_id=posting.rule_id,
        account_full_path=posting.account.full_path if posting.account else None,
    )


def _build_transaction_response(txn: Transaction) -> TransactionResponse:
    return TransactionResponse(
        id=txn.id,
        date=txn.date,
        description=txn.description,
        status=txn.status,
        import_batch_id=txn.import_batch_id,
        created_at=txn.created_at,
        postings=[_build_posting_response(p) for p in txn.postings],
    )


def transaction_filters(
    status: str | None = None,
    account_id: int | None = None,
    category_id: int | None = None,
    batch_id: int | None = None,
    date_from: date | None = None,
    date_to: date | None = None,
    search: str | None = None,
    db: Session = Depends(get_db),
) -> list:
    """The filter query parameters as SQL conditions, shared by the list and its count."""
    conditions = []
    if status:
        conditions.append(Transaction.status == status)
    if date_from:
        conditions.append(Transaction.date >= date_from)
    if date_to:
        conditions.append(Transaction.date <= date_to)
    if search:
        conditions.append(Transaction.description.ilike(f"%{search}%"))
    if account_id:
        conditions.append(
            Transaction.id.in_(
                select(Posting.transaction_id).where(Posting.account_id == account_id)
            )
        )
    if batch_id:
        conditions.append(Transaction.import_batch_id == batch_id)
    if category_id:
        # The category and everything beneath it, so Food includes Food:Groceries
        category = db.get(Account, category_id)
        if category is None:
            raise HTTPException(status_code=404, detail="Category not found")
        conditions.append(
            Transaction.id.in_(
                select(Posting.transaction_id).join(Account).where(in_account(category))
            )
        )
    return conditions


@router.get("", response_model=list[TransactionResponse])
def list_transactions(
    conditions: list = Depends(transaction_filters),
    page: int = Query(default=1, ge=1),
    per_page: int = Query(default=50, ge=1, le=200),
    db: Session = Depends(get_db),
):
    query = (
        select(Transaction)
        .options(joinedload(Transaction.postings).joinedload(Posting.account))
        .where(*conditions)
        .order_by(Transaction.date.desc(), Transaction.id.desc())
    )

    offset = (page - 1) * per_page
    query = query.offset(offset).limit(per_page)

    result = db.execute(query)
    transactions = result.unique().scalars().all()
    return [_build_transaction_response(t) for t in transactions]


@router.get("/count")
def count_transactions(
    conditions: list = Depends(transaction_filters),
    db: Session = Depends(get_db),
):
    count = db.execute(select(func.count(Transaction.id)).where(*conditions)).scalar()
    return {"count": count}


@router.get("/{transaction_id}", response_model=TransactionResponse)
def get_transaction(transaction_id: int, db: Session = Depends(get_db)):
    txn = db.execute(
        select(Transaction)
        .options(joinedload(Transaction.postings).joinedload(Posting.account))
        .where(Transaction.id == transaction_id)
    ).unique().scalar_one_or_none()
    if not txn:
        raise HTTPException(status_code=404, detail="Transaction not found")
    return _build_transaction_response(txn)


@router.post("", response_model=TransactionResponse, status_code=201)
def create_transaction(payload: TransactionCreate, db: Session = Depends(get_db)):
    total = sum(p.amount for p in payload.postings)
    if total != 0:
        raise HTTPException(
            status_code=400, detail=f"Postings must sum to zero, got {total}"
        )

    if len(payload.postings) < 2:
        raise HTTPException(
            status_code=400, detail="Transaction must have at least 2 postings"
        )

    for p in payload.postings:
        account = db.get(Account, p.account_id)
        if not account:
            raise HTTPException(
                status_code=400, detail=f"Account {p.account_id} not found"
            )

    txn = Transaction(
        date=payload.date,
        description=payload.description,
        status=payload.status,
    )
    db.add(txn)
    db.flush()

    for p in payload.postings:
        posting = Posting(
            transaction_id=txn.id,
            account_id=p.account_id,
            amount=p.amount,
            categorised_by_id=p.categorised_by_id,
            rule_id=p.rule_id,
        )
        db.add(posting)

    db.commit()
    db.refresh(txn)

    txn = db.execute(
        select(Transaction)
        .options(joinedload(Transaction.postings).joinedload(Posting.account))
        .where(Transaction.id == txn.id)
    ).unique().scalar_one()
    return _build_transaction_response(txn)


@router.put("/{transaction_id}", response_model=TransactionResponse)
def update_transaction(
    transaction_id: int, payload: TransactionUpdate, db: Session = Depends(get_db)
):
    txn = db.execute(
        select(Transaction)
        .options(joinedload(Transaction.postings).joinedload(Posting.account))
        .where(Transaction.id == transaction_id)
    ).unique().scalar_one_or_none()
    if not txn:
        raise HTTPException(status_code=404, detail="Transaction not found")

    update_data = payload.model_dump(exclude_unset=True)
    if "status" in update_data and update_data["status"] not in ("pending", "confirmed"):
        raise HTTPException(status_code=400, detail="Status must be 'pending' or 'confirmed'")

    for key, value in update_data.items():
        setattr(txn, key, value)

    db.commit()
    db.refresh(txn)
    return _build_transaction_response(txn)


@router.delete("/{transaction_id}", status_code=204)
def delete_transaction(transaction_id: int, db: Session = Depends(get_db)):
    txn = db.get(Transaction, transaction_id)
    if not txn:
        raise HTTPException(status_code=404, detail="Transaction not found")
    remove_transaction(db, txn)
    db.commit()


@router.post("/{transaction_id}/confirm", response_model=TransactionResponse)
def confirm_transaction(transaction_id: int, db: Session = Depends(get_db)):
    txn = db.execute(
        select(Transaction)
        .options(joinedload(Transaction.postings).joinedload(Posting.account))
        .where(Transaction.id == transaction_id)
    ).unique().scalar_one_or_none()
    if not txn:
        raise HTTPException(status_code=404, detail="Transaction not found")
    txn.status = "confirmed"
    db.commit()
    db.refresh(txn)
    return _build_transaction_response(txn)


@router.post("/bulk-confirm")
def bulk_confirm_transactions(
    transaction_ids: list[int], db: Session = Depends(get_db)
):
    transactions = db.execute(
        select(Transaction).where(Transaction.id.in_(transaction_ids))
    ).scalars().all()

    confirmed = 0
    for txn in transactions:
        if txn.status == "pending":
            txn.status = "confirmed"
            confirmed += 1

    db.commit()
    return {"confirmed": confirmed}
