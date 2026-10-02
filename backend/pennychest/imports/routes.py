import hashlib
import os
from datetime import timedelta

from fastapi import APIRouter, BackgroundTasks, Depends, File, Form, HTTPException, UploadFile
from fastapi.responses import FileResponse
from sqlalchemy import and_, func, select
from sqlalchemy.orm import Session, aliased, joinedload

from pennychest.accounts.models import Account, AccountType
from pennychest.ai.config import get_auto_categorise, get_task
from pennychest.ai.learned import learned_enabled
from pennychest.ai.routes import CategoriseOutcome, categorise_uncategorised
from pennychest.core.config import settings
from pennychest.core.database import get_db
from pennychest.core.lookup_models import CategorisationSource, ImportSourceType
from pennychest.imports.base import discover_importers, find_importer, get_importer
from pennychest.imports.models import ImportBatch, RawImportRow
from pennychest.imports.schemas import (
    DetectResponse,
    ImportBatchListResponse,
    ImportBatchResponse,
    ImporterInfo,
    ImportReviewResponse,
    DuplicateOf,
    ImportTransactionResponse,
    MarkAsTransferRequest,
    StatementDetection,
    TransferSuggestion,
)
from pennychest.ai.insights import insights_enabled
from pennychest.ai.insights import run_in_background as insights_in_background
from pennychest.imports.matching import NewRow, describe_source, flag_duplicates, link_transfers
from pennychest.rules.engine import (
    _get_match_type_map,
    find_matching_rule,
    get_all_rules_sorted,
)
from pennychest.taps.service import reconcile as reconcile_taps
from pennychest.transactions.models import Posting, Transaction

router = APIRouter(prefix="/api/imports", tags=["imports"])


@router.post("/detect", response_model=DetectResponse)
def detect_import(
    file: UploadFile = File(...),
    db: Session = Depends(get_db),
):
    """Work out which importer handles a file and, for statements that identify their
    account, which account it belongs to.

    Stateless — does not persist anything. Call this before the upload step.
    """
    file_content = file.file.read()
    filename = file.filename or ""
    importer = find_importer(file_content, filename)
    if importer is None:
        supported = sorted(
            {f".{ext}" for imp in discover_importers().values() for ext in imp.file_types}
        )
        raise HTTPException(
            status_code=422,
            detail=f"No installed importer handles {filename or 'this file'}. "
            f"Supported file types: {', '.join(supported) or 'none'}.",
        )

    info = importer.detect_statement_info(file_content)
    if info is None:
        return DetectResponse(importer=importer.name, statement=None)

    suggested_account = db.execute(
        select(Account).where(Account.bank_identifier == info["bank_identifier"])
    ).scalar_one_or_none()

    return DetectResponse(
        importer=importer.name,
        statement=StatementDetection(
            statement_type=info["statement_type"],
            bank_identifier=info["bank_identifier"],
            label=info["label"],
            name_hint=info["name_hint"],
            suggested_account_id=suggested_account.id if suggested_account else None,
            suggested_account_name=suggested_account.full_path if suggested_account else None,
            period_start=info["period_start"],
            period_end=info["period_end"],
            opening_balance=(
                str(info["opening_balance"]) if info["opening_balance"] is not None else None
            ),
            closing_balance=(
                str(info["closing_balance"]) if info["closing_balance"] is not None else None
            ),
        ),
    )


def _get_transfers_pending_account(db: Session) -> Account | None:
    return db.execute(
        select(Account).where(Account.full_path == "Assets:Transfers:Pending")
    ).scalar_one_or_none()


def _get_uncategorised_account(db: Session) -> Account | None:
    """Get the Expenses:Uncategorised account, creating it if needed."""
    account = db.execute(
        select(Account).where(Account.full_path == "Expenses:Uncategorised")
    ).scalar_one_or_none()

    if not account:
        # Find the Expenses parent
        expenses = db.execute(
            select(Account).where(Account.full_path == "Expenses")
        ).scalar_one_or_none()

        account = Account(
            name="Uncategorised",
            full_path="Expenses:Uncategorised",
            parent_id=expenses.id if expenses else None,
            type="expense",
        )
        db.add(account)
        db.flush()

    return account


@router.get("/importers", response_model=list[ImporterInfo])
def list_importers():
    """List all available importers."""
    importers = discover_importers()
    return [
        ImporterInfo(
            name=name,
            label=imp.label or name,
            description=imp.description,
            file_types=imp.file_types,
        )
        for name, imp in importers.items()
    ]


@router.get("/batches/{batch_id}/file")
def download_batch_file(batch_id: int, db: Session = Depends(get_db)):
    """Download the original uploaded file for an import batch."""
    batch = db.get(ImportBatch, batch_id)
    if not batch or not batch.file_path:
        raise HTTPException(status_code=404, detail="File not found")
    if not os.path.exists(batch.file_path):
        raise HTTPException(status_code=404, detail="File no longer available on disk")
    return FileResponse(
        path=batch.file_path,
        filename=batch.file_name or f"statement-{batch_id}.pdf",
        media_type="application/pdf",
    )


@router.post("/upload")
def upload_import(
    background: BackgroundTasks,
    file: UploadFile = File(...),
    account_id: int = Form(...),
    importer_name: str = Form(default="csv"),
    db: Session = Depends(get_db),
):
    """Upload a file and import transactions.

    Creates an import batch, parses the file, applies rules,
    and creates pending transactions.
    """
    # Validate account
    account = db.get(Account, account_id)
    if not account:
        raise HTTPException(status_code=400, detail="Account not found")

    # Get importer
    importer = get_importer(importer_name)
    if not importer:
        raise HTTPException(status_code=400, detail=f"Importer '{importer_name}' isn't installed")

    # Read file content
    file_content = file.file.read()
    file_hash = hashlib.sha256(file_content).hexdigest()

    # Check for duplicate file
    existing = db.execute(
        select(ImportBatch).where(ImportBatch.file_hash == file_hash)
    ).scalar_one_or_none()
    if existing:
        raise HTTPException(
            status_code=409,
            detail=f"This file has already been imported (batch #{existing.id})",
        )

    # Parse the file
    try:
        parsed_rows = importer.parse(file_content)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))

    if not parsed_rows:
        raise HTTPException(status_code=400, detail="No transactions found in file")

    # Save file to disk
    upload_dir = settings.upload_dir
    os.makedirs(upload_dir, exist_ok=True)
    file_path = os.path.join(upload_dir, f"{file_hash}_{file.filename}")
    with open(file_path, "wb") as f:
        f.write(file_content)

    # Get lookup IDs
    file_source_type = db.execute(
        select(ImportSourceType).where(ImportSourceType.name == "file")
    ).scalar_one()
    rule_source = db.execute(
        select(CategorisationSource).where(CategorisationSource.name == "rule")
    ).scalar_one()
    import_default_source = db.execute(
        select(CategorisationSource).where(CategorisationSource.name == "import_default")
    ).scalar_one()
    transfer_source = db.execute(
        select(CategorisationSource).where(CategorisationSource.name == "transfer")
    ).scalar_one_or_none()

    # Get uncategorised account for unmatched transactions
    uncategorised_account = _get_uncategorised_account(db)
    transfers_pending_account = _get_transfers_pending_account(db)

    # Load rules and match type map
    rules = get_all_rules_sorted(db)
    match_type_map = _get_match_type_map(db)

    # For statements that carry a summary (period + balances), capture it
    # so the batch record reflects what the importer parsed from the file.
    period_start = period_end = None
    opening_balance = closing_balance = None
    info = importer.detect_statement_info(file_content)
    if info is not None:
        period_start = info["period_start"]
        period_end = info["period_end"]
        opening_balance = info["opening_balance"]
        closing_balance = info["closing_balance"]

    # Create import batch
    batch = ImportBatch(
        importer_name=importer_name,
        source_type_id=file_source_type.id,
        file_path=file_path,
        file_hash=file_hash,
        file_name=file.filename,
        account_id=account_id,
        period_start=period_start,
        period_end=period_end,
        opening_balance=opening_balance,
        closing_balance=closing_balance,
    )
    db.add(batch)
    db.flush()

    # Create transactions from parsed rows
    categorised_count = 0
    # Track uncategorised transactions for Layer 3 auto-resolution
    uncategorised_pending: list[tuple[Transaction, Posting, Posting]] = []
    created: list[NewRow] = []

    # Pre-load account types for transfer detection (avoids per-row DB lookups)
    all_accounts_map: dict[int, Account] = {
        a.id: a for a in db.execute(select(Account)).scalars().all()
    }

    for parsed_row in parsed_rows:
        # Find matching rule
        match = find_matching_rule(parsed_row.description, rules, match_type_map)

        is_transfer_rule = False
        if match:
            target_account = all_accounts_map.get(match.target_account_id)
            # If the rule points to an asset/liability account that isn't the
            # import account itself, treat it as a transfer rule: route via
            # Transfers:Pending so the other side can be auto-resolved later.
            if (
                transfers_pending_account
                and transfer_source
                and target_account
                and target_account.type in ("asset", "liability")
                and match.target_account_id != account_id
                and match.target_account_id != transfers_pending_account.id
            ):
                is_transfer_rule = True
                target_account_id = transfers_pending_account.id
                categorised_by_id = transfer_source.id
                rule_id = match.rule_id
            else:
                target_account_id = match.target_account_id
                categorised_by_id = rule_source.id
                rule_id = match.rule_id
            categorised_count += 1
        else:
            target_account_id = uncategorised_account.id
            categorised_by_id = import_default_source.id
            rule_id = None

        # Create the transaction
        txn = Transaction(
            date=parsed_row.date,
            description=parsed_row.description,
            status="pending",
            import_batch_id=batch.id,
            currency=account.currency,
            import_metadata=parsed_row.metadata if parsed_row.metadata else None,
        )
        db.add(txn)
        db.flush()

        # Create the two postings (double-entry)
        # Posting 1: the bank account side
        posting_bank = Posting(
            transaction_id=txn.id,
            account_id=account_id,
            amount=parsed_row.amount,
            categorised_by_id=import_default_source.id,
        )
        db.add(posting_bank)

        # Posting 2: the categorised account side (opposite sign)
        posting_category = Posting(
            transaction_id=txn.id,
            account_id=target_account_id,
            amount=-parsed_row.amount,
            categorised_by_id=categorised_by_id,
            rule_id=rule_id,
        )
        db.add(posting_category)
        db.flush()
        created.append(NewRow(txn, posting_bank, posting_category))

        # Queue for Layer 3 auto-resolution if uncategorised OR routed via a
        # transfer rule (both need checking against Transfers:Pending).
        if not match or is_transfer_rule:
            uncategorised_pending.append((txn, posting_bank, posting_category))

        # Store the raw import row
        raw_row = RawImportRow(
            batch_id=batch.id,
            raw_content=parsed_row.raw_content,
            line_number=parsed_row.line_number,
            transaction_id=txn.id,
        )
        db.add(raw_row)

    # Layer 3: auto-resolve transfers — for each uncategorised transaction,
    # check if there is an existing unlinked Transfers:Pending posting with
    # the opposite amount on a different account within ±3 days.
    if transfers_pending_account and transfer_source:
        for (txn, posting_bank, posting_category) in uncategorised_pending:
            target_amount = -posting_bank.amount
            date_min = txn.date - timedelta(days=3)
            date_max = txn.date + timedelta(days=3)

            ExistingTxn = aliased(Transaction)
            PendingPosting = aliased(Posting)
            BankPosting = aliased(Posting)

            candidate = db.execute(
                select(ExistingTxn)
                .join(PendingPosting, PendingPosting.transaction_id == ExistingTxn.id)
                .join(
                    BankPosting,
                    and_(
                        BankPosting.transaction_id == ExistingTxn.id,
                        BankPosting.id != PendingPosting.id,
                        BankPosting.amount == target_amount,
                        BankPosting.account_id != account_id,
                    ),
                )
                .where(
                    and_(
                        ExistingTxn.id != txn.id,
                        ExistingTxn.transfer_peer_id.is_(None),
                        ExistingTxn.date >= date_min,
                        ExistingTxn.date <= date_max,
                        PendingPosting.account_id == transfers_pending_account.id,
                    )
                )
                .limit(1)
            ).scalar_one_or_none()

            if candidate is not None:
                posting_category.account_id = transfers_pending_account.id
                posting_category.categorised_by_id = transfer_source.id
                posting_category.rule_id = None
                txn.transfer_peer_id = candidate.id
                candidate.transfer_peer_id = txn.id

    # Statement lines already in the ledger, and transfers whose sides are described
    # differently, need judging rather than exact matching.
    flag_duplicates(db, batch.id, account_id, created)
    if transfers_pending_account:
        link_transfers(db, account_id, created, transfers_pending_account, uncategorised_account)

    # Link any card taps that this statement now covers
    reconcile_taps(db)

    db.commit()
    db.refresh(batch)

    # Then categorise whatever rules and card taps didn't: from the user's own history first,
    # then with AI if that's set up. The import stands even if the AI fails; the error is
    # reported alongside it and kept in the AI logs.
    use_ai = get_auto_categorise(db) and bool(get_task(db, "categorise")[1])
    outcome = CategoriseOutcome()
    if use_ai or learned_enabled(db):
        outcome = categorise_uncategorised(db, batch=batch, source="import", use_ai=use_ai)

    # Scoring charges and labelling subscriptions waits for categories, and the upload
    # shouldn't wait for it.
    if insights_enabled(db):
        background.add_task(insights_in_background, batch.id)

    return {
        "batch_id": batch.id,
        "file_name": file.filename,
        "transaction_count": len(parsed_rows),
        "categorised_count": categorised_count,
        "uncategorised_count": len(parsed_rows) - categorised_count - outcome.total,
        "ai_categorised_count": outcome.total,
        "learned_count": outcome.learned,
        "ai_error": outcome.error,
    }


@router.get("/batches", response_model=list[ImportBatchListResponse])
def list_batches(db: Session = Depends(get_db)):
    """List all import batches with summary counts."""
    batches = db.execute(
        select(ImportBatch).order_by(ImportBatch.imported_at.desc())
    ).scalars().all()

    result = []
    for batch in batches:
        # Count transactions in this batch
        txn_counts = db.execute(
            select(
                func.count(Transaction.id).label("total"),
                func.count(
                    Transaction.id
                ).filter(Transaction.status == "confirmed").label("confirmed"),
                func.count(
                    Transaction.id
                ).filter(Transaction.status == "pending").label("pending"),
            ).where(Transaction.import_batch_id == batch.id)
        ).one()

        acct = db.get(Account, batch.account_id) if batch.account_id else None
        identifier = (acct.bank_identifier or "").strip() if acct else ""

        result.append(
            ImportBatchListResponse(
                id=batch.id,
                importer_name=batch.importer_name,
                file_name=batch.file_name,
                account_id=batch.account_id,
                account_path=acct.full_path if acct else None,
                account_name=acct.name if acct else None,
                account_number_hint=identifier[-4:] or None,
                imported_at=batch.imported_at,
                period_start=batch.period_start,
                period_end=batch.period_end,
                transaction_count=txn_counts.total,
                confirmed_count=txn_counts.confirmed,
                pending_count=txn_counts.pending,
            )
        )

    return result


@router.get("/batches/{batch_id}")
def get_batch_review(batch_id: int, db: Session = Depends(get_db)):
    """Get a batch with all its transactions for review."""
    batch = db.get(ImportBatch, batch_id)
    if not batch:
        raise HTTPException(status_code=404, detail="Import batch not found")

    # Get transactions for this batch with all relationships
    transactions = db.execute(
        select(Transaction)
        .options(joinedload(Transaction.postings).joinedload(Posting.account))
        .options(joinedload(Transaction.postings).joinedload(Posting.rule))
        .where(Transaction.import_batch_id == batch_id)
        .order_by(Transaction.date.desc(), Transaction.id.desc())
    ).unique().scalars().all()

    # Get raw import rows for these transactions
    txn_ids = [t.id for t in transactions]
    raw_rows = {}
    if txn_ids:
        rows = db.execute(
            select(RawImportRow).where(RawImportRow.transaction_id.in_(txn_ids))
        ).scalars().all()
        raw_rows = {r.transaction_id: r for r in rows}

    # Get categorisation source names
    cat_sources = db.execute(select(CategorisationSource)).scalars().all()
    cat_source_map = {c.id: c.name for c in cat_sources}

    # Build response
    txn_responses = []
    for txn in transactions:
        # Find the bank-side and category-side postings
        bank_posting = None
        category_posting = None
        for p in txn.postings:
            if batch.account_id and p.account_id == batch.account_id:
                bank_posting = p
            else:
                category_posting = p

        # If we can't distinguish, use first two postings
        if not bank_posting and not category_posting and len(txn.postings) >= 2:
            bank_posting = txn.postings[0]
            category_posting = txn.postings[1]

        raw_row = raw_rows.get(txn.id)

        txn_responses.append(
            ImportTransactionResponse(
                id=txn.id,
                date=txn.date.isoformat(),
                description=txn.description,
                amount=str(bank_posting.amount) if bank_posting else "0",
                status=txn.status,
                account_full_path=(
                    bank_posting.account.full_path if bank_posting and bank_posting.account else None
                ),
                target_account_full_path=(
                    category_posting.account.full_path
                    if category_posting and category_posting.account
                    else None
                ),
                categorised_by=(
                    cat_source_map.get(category_posting.categorised_by_id)
                    if category_posting
                    else None
                ),
                rule_id=category_posting.rule_id if category_posting else None,
                rule_pattern=(
                    category_posting.rule.pattern
                    if category_posting and category_posting.rule
                    else None
                ),
                raw_content=raw_row.raw_content if raw_row else None,
                line_number=raw_row.line_number if raw_row else None,
                transfer_peer_id=txn.transfer_peer_id,
                import_batch_id=txn.import_batch_id,
                source_file_name=batch.file_name,
                duplicate_of=_duplicate_of(txn),
                unusual_score=txn.unusual_score,
            )
        )

    # Count categorised
    categorised_count = sum(
        1 for t in txn_responses if t.categorised_by == "rule"
    )

    batch_response = ImportBatchResponse(
        id=batch.id,
        importer_name=batch.importer_name,
        source_type_id=batch.source_type_id,
        file_path=batch.file_path,
        file_hash=batch.file_hash,
        file_name=batch.file_name,
        account_id=batch.account_id,
        imported_at=batch.imported_at,
        transaction_count=len(txn_responses),
        categorised_count=categorised_count,
    )

    return ImportReviewResponse(
        batch=batch_response,
        transactions=txn_responses,
    )


@router.post("/batches/{batch_id}/confirm-all")
def confirm_all_in_batch(batch_id: int, db: Session = Depends(get_db)):
    """Confirm all pending transactions in a batch."""
    batch = db.get(ImportBatch, batch_id)
    if not batch:
        raise HTTPException(status_code=404, detail="Import batch not found")

    transactions = db.execute(
        select(Transaction)
        .where(Transaction.import_batch_id == batch_id)
        .where(Transaction.status == "pending")
    ).scalars().all()

    confirmed = 0
    for txn in transactions:
        txn.status = "confirmed"
        confirmed += 1

    db.commit()
    return {"confirmed": confirmed}


@router.post("/batches/{batch_id}/reapply-rules")
def reapply_rules_to_batch(batch_id: int, db: Session = Depends(get_db)):
    """Re-apply rules to all transactions in a batch.

    Only re-categorises postings that were set by rules or import_default.
    Manual categorisations are preserved.
    """
    batch = db.get(ImportBatch, batch_id)
    if not batch:
        raise HTTPException(status_code=404, detail="Import batch not found")

    rules = get_all_rules_sorted(db)
    match_type_map = _get_match_type_map(db)

    rule_source = db.execute(
        select(CategorisationSource).where(CategorisationSource.name == "rule")
    ).scalar_one()
    import_default_source = db.execute(
        select(CategorisationSource).where(CategorisationSource.name == "import_default")
    ).scalar_one()
    manual_source = db.execute(
        select(CategorisationSource).where(CategorisationSource.name == "manual")
    ).scalar_one()
    transfer_source = db.execute(
        select(CategorisationSource).where(CategorisationSource.name == "transfer")
    ).scalar_one_or_none()

    uncategorised_account = _get_uncategorised_account(db)

    transactions = db.execute(
        select(Transaction)
        .options(joinedload(Transaction.postings))
        .where(Transaction.import_batch_id == batch_id)
    ).unique().scalars().all()

    updated = 0
    for txn in transactions:
        # Find the category posting (not the bank account posting)
        category_posting = None
        for p in txn.postings:
            if batch.account_id and p.account_id != batch.account_id:
                category_posting = p
                break

        if not category_posting:
            continue

        # Skip manual and transfer categorisations
        if category_posting.categorised_by_id == manual_source.id:
            continue
        if transfer_source and category_posting.categorised_by_id == transfer_source.id:
            continue

        # Re-apply rules
        match = find_matching_rule(txn.description, rules, match_type_map)
        if match:
            if (
                category_posting.account_id != match.target_account_id
                or category_posting.rule_id != match.rule_id
            ):
                category_posting.account_id = match.target_account_id
                category_posting.categorised_by_id = rule_source.id
                category_posting.rule_id = match.rule_id
                updated += 1
        else:
            if category_posting.categorised_by_id != import_default_source.id:
                category_posting.account_id = uncategorised_account.id
                category_posting.categorised_by_id = import_default_source.id
                category_posting.rule_id = None
                updated += 1

    db.commit()
    return {"updated": updated}


@router.put("/transactions/{transaction_id}/categorise")
def categorise_transaction(
    transaction_id: int,
    target_account_id: int,
    db: Session = Depends(get_db),
):
    """Manually categorise a transaction (imported or added by hand)."""
    txn = db.execute(
        select(Transaction)
        .options(joinedload(Transaction.postings).joinedload(Posting.account))
        .where(Transaction.id == transaction_id)
    ).unique().scalar_one_or_none()
    if not txn:
        raise HTTPException(status_code=404, detail="Transaction not found")

    # Validate target account
    target_account = db.get(Account, target_account_id)
    if not target_account:
        raise HTTPException(status_code=400, detail="Target account not found")

    manual_source = db.execute(
        select(CategorisationSource).where(CategorisationSource.name == "manual")
    ).scalar_one()

    # Find the category posting (the one that's not the bank account)
    # For imported transactions, the bank account posting has categorised_by = import_default
    # and matches the import batch's account_id
    batch_account_id = txn.import_batch.account_id if txn.import_batch else None

    # The spending or income side, when there is exactly one; otherwise fall back to telling
    # the sides apart by the statement's account.
    category_sides = [
        p for p in txn.postings if p.account.type in (AccountType.EXPENSE, AccountType.INCOME)
    ]
    category_posting = category_sides[0] if len(category_sides) == 1 else None
    for p in txn.postings if category_posting is None else []:
        if batch_account_id and p.account_id != batch_account_id:
            category_posting = p
            break
        elif not batch_account_id and p.account_id != txn.postings[0].account_id:
            category_posting = p
            break

    if not category_posting and len(txn.postings) >= 2:
        category_posting = txn.postings[1]

    if not category_posting:
        raise HTTPException(status_code=400, detail="Cannot find posting to categorise")

    category_posting.account_id = target_account_id
    category_posting.categorised_by_id = manual_source.id
    category_posting.rule_id = None

    db.commit()
    return {"status": "ok", "transaction_id": transaction_id}


@router.get(
    "/transactions/{transaction_id}/transfer-suggestions",
    response_model=list[TransferSuggestion],
)
def get_transfer_suggestions(transaction_id: int, db: Session = Depends(get_db)):
    """Find candidate transactions that could be the other side of a transfer."""
    txn = db.execute(
        select(Transaction)
        .options(joinedload(Transaction.postings))
        .where(Transaction.id == transaction_id)
    ).unique().scalar_one_or_none()
    if not txn:
        raise HTTPException(status_code=404, detail="Transaction not found")

    batch_account_id = txn.import_batch.account_id if txn.import_batch else None
    bank_posting = None
    for p in txn.postings:
        if batch_account_id and p.account_id == batch_account_id:
            bank_posting = p
            break
    if not bank_posting and txn.postings:
        bank_posting = txn.postings[0]
    if not bank_posting:
        return []

    target_amount = -bank_posting.amount
    date_min = txn.date - timedelta(days=3)
    date_max = txn.date + timedelta(days=3)

    CandidateTxn = aliased(Transaction)
    BankPosting = aliased(Posting)

    rows = db.execute(
        select(CandidateTxn, BankPosting)
        .join(BankPosting, BankPosting.transaction_id == CandidateTxn.id)
        .join(Account, BankPosting.account_id == Account.id)
        .where(
            and_(
                CandidateTxn.id != transaction_id,
                CandidateTxn.transfer_peer_id.is_(None),
                CandidateTxn.date >= date_min,
                CandidateTxn.date <= date_max,
                BankPosting.amount == target_amount,
                Account.type.in_(["asset", "liability"]),
                BankPosting.account_id != batch_account_id,
            )
        )
        .limit(10)
    ).all()

    suggestions = []
    for candidate_txn, candidate_bank_posting in rows:
        candidate_account = db.get(Account, candidate_bank_posting.account_id)
        days_apart = abs((candidate_txn.date - txn.date).days)
        suggestions.append(TransferSuggestion(
            transaction_id=candidate_txn.id,
            date=candidate_txn.date.isoformat(),
            description=candidate_txn.description,
            amount=str(candidate_bank_posting.amount),
            account_full_path=candidate_account.full_path if candidate_account else None,
            days_apart=days_apart,
        ))

    suggestions.sort(key=lambda s: s.days_apart)
    return suggestions


def _duplicate_of(txn: Transaction) -> DuplicateOf | None:
    original = txn.duplicate_of
    if original is None:
        return None
    return DuplicateOf(
        transaction_id=original.id,
        date=original.date.isoformat(),
        description=original.description,
        source=describe_source(original),
    )


@router.post("/transactions/{transaction_id}/not-duplicate", status_code=204)
def mark_not_duplicate(transaction_id: int, db: Session = Depends(get_db)):
    """Keep a transaction that was flagged as a possible duplicate. (Deleting it is done
    like any other transaction.)"""
    txn = db.get(Transaction, transaction_id)
    if not txn:
        raise HTTPException(status_code=404, detail="Transaction not found")
    txn.duplicate_of_id = None
    db.commit()


@router.post("/transactions/{transaction_id}/mark-as-transfer")
def mark_as_transfer(
    transaction_id: int,
    payload: MarkAsTransferRequest,
    db: Session = Depends(get_db),
):
    """Mark a transaction as a transfer, parking it in Assets:Transfers:Pending.

    If peer_transaction_id is provided, links both transactions symmetrically.
    Otherwise parks this transaction solo (to be resolved when the other side
    is imported).
    """
    txn = db.execute(
        select(Transaction)
        .options(joinedload(Transaction.postings).joinedload(Posting.account))
        .where(Transaction.id == transaction_id)
    ).unique().scalar_one_or_none()
    if not txn:
        raise HTTPException(status_code=404, detail="Transaction not found")

    transfers_pending = _get_transfers_pending_account(db)
    if not transfers_pending:
        raise HTTPException(
            status_code=500,
            detail="Assets:Transfers:Pending account not found. Load UK defaults first.",
        )

    transfer_source = db.execute(
        select(CategorisationSource).where(CategorisationSource.name == "transfer")
    ).scalar_one()

    batch_account_id = txn.import_batch.account_id if txn.import_batch else None
    category_posting = None
    for p in txn.postings:
        if batch_account_id and p.account_id != batch_account_id:
            category_posting = p
            break
    if not category_posting and len(txn.postings) >= 2:
        category_posting = txn.postings[1]
    if not category_posting:
        raise HTTPException(status_code=400, detail="Cannot find category posting")

    category_posting.account_id = transfers_pending.id
    category_posting.categorised_by_id = transfer_source.id
    category_posting.rule_id = None

    peer_transaction_id = None

    if payload.peer_transaction_id is not None:
        peer_txn = db.execute(
            select(Transaction)
            .options(joinedload(Transaction.postings))
            .where(Transaction.id == payload.peer_transaction_id)
        ).unique().scalar_one_or_none()
        if not peer_txn:
            raise HTTPException(status_code=404, detail="Peer transaction not found")
        if peer_txn.transfer_peer_id is not None:
            raise HTTPException(
                status_code=409, detail="Peer transaction is already linked to a transfer"
            )

        peer_batch_account_id = peer_txn.import_batch.account_id if peer_txn.import_batch else None
        peer_category_posting = None
        for p in peer_txn.postings:
            if peer_batch_account_id and p.account_id != peer_batch_account_id:
                peer_category_posting = p
                break
        if not peer_category_posting and len(peer_txn.postings) >= 2:
            peer_category_posting = peer_txn.postings[1]

        if peer_category_posting:
            peer_category_posting.account_id = transfers_pending.id
            peer_category_posting.categorised_by_id = transfer_source.id
            peer_category_posting.rule_id = None

        txn.transfer_peer_id = peer_txn.id
        peer_txn.transfer_peer_id = txn.id
        peer_transaction_id = peer_txn.id

    db.commit()
    return {
        "status": "ok",
        "transaction_id": transaction_id,
        "peer_transaction_id": peer_transaction_id,
        "parked": peer_transaction_id is None,
    }


@router.get("/transfers/pending-balance")
def get_pending_transfers_balance(db: Session = Depends(get_db)):
    """Return the balance of Assets:Transfers:Pending and count of unlinked transactions."""
    transfers_pending = _get_transfers_pending_account(db)
    if not transfers_pending:
        return {"balance": "0", "unlinked_count": 0}

    balance = db.execute(
        select(func.coalesce(func.sum(Posting.amount), 0))
        .where(Posting.account_id == transfers_pending.id)
    ).scalar()

    unlinked_count = db.execute(
        select(func.count(Transaction.id.distinct()))
        .join(Posting, Posting.transaction_id == Transaction.id)
        .where(
            and_(
                Posting.account_id == transfers_pending.id,
                Transaction.transfer_peer_id.is_(None),
            )
        )
    ).scalar()

    return {
        "balance": str(balance),
        "unlinked_count": int(unlinked_count or 0),
    }


@router.get("/account/{account_id}")
def get_account_review(account_id: int, db: Session = Depends(get_db)):
    """Get all import batches and transactions for an account, ordered by statement period."""
    from pennychest.imports.schemas import AccountBatchSummary, AccountReviewResponse

    account = db.get(Account, account_id)
    if not account:
        raise HTTPException(status_code=404, detail="Account not found")

    batches = db.execute(
        select(ImportBatch)
        .where(ImportBatch.account_id == account_id)
        .order_by(ImportBatch.period_start.asc().nullslast(), ImportBatch.imported_at.asc())
    ).scalars().all()

    cat_sources = db.execute(select(CategorisationSource)).scalars().all()
    cat_source_map = {c.id: c.name for c in cat_sources}

    batch_summaries = []
    all_transactions = []

    for batch in batches:
        transactions = db.execute(
            select(Transaction)
            .options(joinedload(Transaction.postings).joinedload(Posting.account))
            .options(joinedload(Transaction.postings).joinedload(Posting.rule))
            .where(Transaction.import_batch_id == batch.id)
            .order_by(Transaction.date.desc(), Transaction.id.desc())
        ).unique().scalars().all()

        txn_ids = [t.id for t in transactions]
        raw_rows: dict = {}
        if txn_ids:
            rows = db.execute(
                select(RawImportRow).where(RawImportRow.transaction_id.in_(txn_ids))
            ).scalars().all()
            raw_rows = {r.transaction_id: r for r in rows}

        pending_count = sum(1 for t in transactions if t.status == "pending")

        batch_summaries.append(AccountBatchSummary(
            id=batch.id,
            file_name=batch.file_name,
            imported_at=batch.imported_at,
            period_start=batch.period_start.isoformat() if batch.period_start else None,
            period_end=batch.period_end.isoformat() if batch.period_end else None,
            transaction_count=len(transactions),
            pending_count=pending_count,
        ))

        for txn in transactions:
            bank_posting = None
            category_posting = None
            for p in txn.postings:
                if batch.account_id and p.account_id == batch.account_id:
                    bank_posting = p
                else:
                    category_posting = p
            if not bank_posting and not category_posting and len(txn.postings) >= 2:
                bank_posting = txn.postings[0]
                category_posting = txn.postings[1]

            raw_row = raw_rows.get(txn.id)
            all_transactions.append(
                ImportTransactionResponse(
                    id=txn.id,
                    date=txn.date.isoformat(),
                    description=txn.description,
                    amount=str(bank_posting.amount) if bank_posting else "0",
                    status=txn.status,
                    account_full_path=(
                        bank_posting.account.full_path
                        if bank_posting and bank_posting.account
                        else None
                    ),
                    target_account_full_path=(
                        category_posting.account.full_path
                        if category_posting and category_posting.account
                        else None
                    ),
                    categorised_by=(
                        cat_source_map.get(category_posting.categorised_by_id)
                        if category_posting
                        else None
                    ),
                    rule_id=category_posting.rule_id if category_posting else None,
                    rule_pattern=(
                        category_posting.rule.pattern
                        if category_posting and category_posting.rule
                        else None
                    ),
                    raw_content=raw_row.raw_content if raw_row else None,
                    line_number=raw_row.line_number if raw_row else None,
                    transfer_peer_id=txn.transfer_peer_id,
                    import_batch_id=txn.import_batch_id,
                    source_file_name=batch.file_name,
                    duplicate_of=_duplicate_of(txn),
                    unusual_score=txn.unusual_score,
                )
            )

    return AccountReviewResponse(
        account_id=account_id,
        account_path=account.full_path,
        batches=batch_summaries,
        transactions=all_transactions,
    )


@router.post("/account/{account_id}/confirm-all")
def confirm_all_for_account(account_id: int, db: Session = Depends(get_db)):
    """Confirm all pending transactions across all batches for an account."""
    batches = db.execute(
        select(ImportBatch).where(ImportBatch.account_id == account_id)
    ).scalars().all()
    batch_ids = [b.id for b in batches]

    transactions = db.execute(
        select(Transaction)
        .where(Transaction.import_batch_id.in_(batch_ids))
        .where(Transaction.status == "pending")
    ).scalars().all()

    confirmed = 0
    for txn in transactions:
        txn.status = "confirmed"
        confirmed += 1

    db.commit()
    return {"confirmed": confirmed}


@router.delete("/batches/{batch_id}")
def delete_batch(batch_id: int, db: Session = Depends(get_db)):
    """Delete an import batch and all its transactions."""
    batch = db.get(ImportBatch, batch_id)
    if not batch:
        raise HTTPException(status_code=404, detail="Import batch not found")

    # Delete raw import rows
    db.execute(
        RawImportRow.__table__.delete().where(RawImportRow.batch_id == batch_id)
    )

    # Delete transactions and their postings (cascade)
    transactions = db.execute(
        select(Transaction).where(Transaction.import_batch_id == batch_id)
    ).scalars().all()
    for txn in transactions:
        db.delete(txn)

    db.delete(batch)
    db.commit()
    return {"deleted": True}
