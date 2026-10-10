"""CSV importer for bank statement files.

Recognises common UK bank statement layouts by their column names. Others are imported with a
column mapping: chosen by the user, suggested by a model, or saved as a template (see
csv_mapping)."""

import csv
import io
from datetime import date, datetime
from decimal import Decimal, InvalidOperation

from pennychest.imports.base import BaseImporter, ImportedRow

# Common column name variations
DATE_COLUMNS = {"date", "transaction date", "trans date", "value date", "posted date", "booking date"}
DESCRIPTION_COLUMNS = {
    "description",
    "narrative",
    "details",
    "transaction description",
    "memo",
    "reference",
    "payee",
    "name",
}
AMOUNT_COLUMNS = {"amount", "value", "transaction amount"}
DEBIT_COLUMNS = {"debit", "debit amount", "money out", "paid out", "withdrawals"}
CREDIT_COLUMNS = {"credit", "credit amount", "money in", "paid in", "deposits"}

# Common date formats to try
DATE_FORMATS = [
    "%d/%m/%Y",
    "%Y-%m-%d",
    "%d-%m-%Y",
    "%d/%m/%y",
    "%d-%m-%y",
    "%Y/%m/%d",
    "%m/%d/%Y",
    "%d %b %Y",
    "%d %B %Y",
]


def _normalise_header(header: str) -> str:
    """Normalise a column header for matching."""
    return header.strip().lower().replace("_", " ")


def _find_column(headers: list[str], candidates: set[str]) -> int | None:
    """Find the index of a column matching any of the candidate names."""
    for i, header in enumerate(headers):
        if _normalise_header(header) in candidates:
            return i
    return None


def _parse_date(value: str) -> date:
    """Try parsing a date string with common formats."""
    value = value.strip()
    for fmt in DATE_FORMATS:
        try:
            return datetime.strptime(value, fmt).date()
        except ValueError:
            continue
    raise ValueError(f"Could not parse date: {value!r}")


def _parse_amount(value: str) -> Decimal:
    """Parse a monetary amount string to Decimal."""
    value = value.strip().replace(",", "").replace("£", "").replace("$", "").replace("€", "")
    if not value or value == "-":
        return Decimal("0")
    return Decimal(value)


# What each column can hold, in the order they're shown
FIELDS = {
    "date": "date",
    "description": "description",
    "amount": "amount",
    "money_out": "money out",
    "money_in": "money in",
}


def read_csv(file_content: bytes) -> tuple[list[str], list[list[str]]]:
    """The header row and the data rows."""
    text = file_content.decode("utf-8-sig")
    rows = list(csv.reader(io.StringIO(text)))
    if not rows or not rows[0]:
        raise ValueError("CSV file is empty or has no header row")
    return rows[0], rows[1:]


def columns_by_name(headers: list[str]) -> dict[str, int]:
    """The columns whose header names a field, e.g. "Paid out" for money out."""
    found = {
        "date": _find_column(headers, DATE_COLUMNS),
        "description": _find_column(headers, DESCRIPTION_COLUMNS),
        "amount": _find_column(headers, AMOUNT_COLUMNS),
        "money_out": _find_column(headers, DEBIT_COLUMNS),
        "money_in": _find_column(headers, CREDIT_COLUMNS),
    }
    return {field: column for field, column in found.items() if column is not None}


def missing_fields(columns: dict[str, int]) -> list[str]:
    """The fields a mapping still needs: a date, a description and some amount."""
    missing = [FIELDS[f] for f in ("date", "description") if columns.get(f) is None]
    if all(columns.get(f) is None for f in ("amount", "money_out", "money_in")):
        missing.append("amount")
    return missing


class CsvImporter(BaseImporter):
    """Generic CSV importer for bank statements.

    Auto-detects column layout by matching header names against known patterns.
    Supports both single-amount and separate debit/credit column formats.
    """

    name = "csv"
    label = "CSV"
    description = "Generic CSV bank statement importer"
    file_types = ["csv", "txt"]

    def detect(self, file_content: bytes, filename: str = "") -> bool:
        if not super().detect(file_content, filename):
            return False
        try:
            text = file_content.decode("utf-8-sig")
            reader = csv.reader(io.StringIO(text))
            headers = next(reader, None)
            if not headers:
                return False
            normalised = [_normalise_header(h) for h in headers]
            has_date = any(n in DATE_COLUMNS for n in normalised)
            has_desc = any(n in DESCRIPTION_COLUMNS for n in normalised)
            has_amount = (
                any(n in AMOUNT_COLUMNS for n in normalised)
                or any(n in DEBIT_COLUMNS for n in normalised)
            )
            return has_date and has_desc and has_amount
        except Exception:
            return False

    def parse(self, file_content: bytes) -> list[ImportedRow]:
        headers, _ = read_csv(file_content)
        columns = columns_by_name(headers)
        missing = missing_fields(columns)
        if missing:
            raise ValueError(
                f"Couldn't tell which columns hold the {' and '.join(missing)}. "
                f"Headers: {headers}."
            )
        return self.parse_mapped(file_content, columns)

    def parse_mapped(self, file_content: bytes, columns: dict[str, int]) -> list[ImportedRow]:
        """Parse with `columns` saying which column (counting from 0) holds each field:
        "date", "description", and "amount" or one or both of "money_out" and "money_in"."""
        missing = missing_fields(columns)
        if missing:
            raise ValueError(f"Choose the columns for the {' and '.join(missing)}.")
        _, data = read_csv(file_content)
        date_col = columns["date"]
        desc_col = columns["description"]
        amount_col = columns.get("amount")
        debit_col = columns.get("money_out")
        credit_col = columns.get("money_in")

        rows: list[ImportedRow] = []
        for line_num, row in enumerate(data, start=2):
            if not row or all(cell.strip() == "" for cell in row):
                continue

            try:
                txn_date = _parse_date(row[date_col])
            except (ValueError, IndexError):
                continue

            try:
                description = row[desc_col].strip()
            except IndexError:
                continue

            if not description:
                continue

            try:
                if amount_col is not None:
                    amount = _parse_amount(row[amount_col])
                else:
                    debit = _parse_amount(row[debit_col]) if debit_col is not None else Decimal("0")
                    credit = (
                        _parse_amount(row[credit_col]) if credit_col is not None else Decimal("0")
                    )
                    # Money out is always outgoing, whether the bank writes it as 12.50
                    # or -12.50
                    amount = credit - abs(debit)
            except (InvalidOperation, IndexError):
                continue

            if amount == 0:
                continue

            raw_content = ",".join(row)
            rows.append(
                ImportedRow(
                    date=txn_date,
                    description=description,
                    amount=amount,
                    raw_content=raw_content,
                    line_number=line_num,
                )
            )

        return rows
