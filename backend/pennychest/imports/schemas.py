from datetime import date, datetime

from pydantic import BaseModel


class ImportBatchResponse(BaseModel):
    id: int
    importer_name: str
    source_type_id: int
    file_path: str | None
    file_hash: str | None
    file_name: str | None
    account_id: int | None
    imported_at: datetime
    transaction_count: int = 0
    categorised_count: int = 0
    # When the matching model last looked for duplicates and transfers in it
    matched_at: datetime | None = None

    model_config = {"from_attributes": True}


class ImportBatchListResponse(BaseModel):
    id: int
    importer_name: str
    file_name: str | None
    account_id: int | None
    account_path: str | None
    # The account's own name, and the last four characters of its bank identifier if it has one
    account_name: str | None = None
    account_number_hint: str | None = None
    imported_at: datetime
    # The period the statement says it covers, when the importer could read it
    period_start: date | None = None
    period_end: date | None = None
    transaction_count: int
    confirmed_count: int
    pending_count: int

    model_config = {"from_attributes": True}


class DuplicateOf(BaseModel):
    """The transaction already in the ledger that an imported one looks like."""

    transaction_id: int
    date: str
    description: str
    # "imported from <file>" or "added by hand"
    source: str


class ImportTransactionResponse(BaseModel):
    id: int
    date: str
    description: str
    amount: str
    status: str
    account_full_path: str | None
    target_account_full_path: str | None
    categorised_by: str | None
    rule_id: int | None
    rule_pattern: str | None
    raw_content: str | None
    line_number: int | None
    transfer_peer_id: int | None = None
    import_batch_id: int | None = None
    source_file_name: str | None = None
    duplicate_of: DuplicateOf | None = None
    # 0 (normal) to 1 (very unusual), once the insights model has judged it
    unusual_score: float | None = None


class TransferSuggestion(BaseModel):
    transaction_id: int
    date: str
    description: str
    amount: str
    account_full_path: str | None
    days_apart: int


class MarkAsTransferRequest(BaseModel):
    peer_transaction_id: int | None = None


class ImportReviewResponse(BaseModel):
    batch: ImportBatchResponse
    transactions: list[ImportTransactionResponse]


class ImporterInfo(BaseModel):
    name: str
    label: str
    description: str
    file_types: list[str]


class StatementDetection(BaseModel):
    """What the importer read from a statement, plus the account it seems to belong to."""

    statement_type: str
    bank_identifier: str
    label: str
    name_hint: str | None
    suggested_account_id: int | None
    suggested_account_name: str | None
    period_start: date | None
    period_end: date | None
    # Signed decimal strings; see StatementInfo for the sign convention
    opening_balance: str | None
    closing_balance: str | None


class DetectResponse(BaseModel):
    importer: str
    # Only for formats that identify their account (e.g. bank PDF statements)
    statement: StatementDetection | None


class AccountBatchSummary(BaseModel):
    id: int
    file_name: str | None
    imported_at: datetime
    period_start: str | None
    period_end: str | None
    transaction_count: int
    pending_count: int


class AccountReviewResponse(BaseModel):
    account_id: int
    account_path: str | None
    batches: list[AccountBatchSummary]
    transactions: list[ImportTransactionResponse]
