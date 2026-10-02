"""BaseImporter interface for plugin-based import system.

Importers are discovered via Python entry points:
    importlib.metadata.entry_points(group='pennychest.importers')

All first-party importers live in the core repo and use the same mechanism.
"""

from abc import ABC, abstractmethod
from dataclasses import dataclass
from datetime import date
from decimal import Decimal
from typing import TypedDict

from pennychest.core.plugins import load_plugins

ENTRY_POINT_GROUP = "pennychest.importers"


@dataclass
class ImportedRow:
    """A single parsed row from an import source."""

    date: date
    description: str
    amount: Decimal
    raw_content: str
    line_number: int | None = None
    metadata: dict = None  # type: ignore[assignment]

    def __post_init__(self) -> None:
        if self.metadata is None:
            self.metadata = {}


class StatementInfo(TypedDict):
    """What an importer can read from a statement before importing it.

    Lets the import page suggest the right account (or offer to create one) and an
    opening balance. Importers for formats that carry no such details don't provide it.
    """

    statement_type: str  # "current_account" | "savings" | "credit_card"
    bank_identifier: str  # e.g. "12-34-56 12345678" or "****1234"
    label: str  # Human-readable, e.g. "HSBC Current Account Statement"
    name_hint: str | None  # Suggested name for a new account, e.g. "HSBC Advance"
    # Statement summary fields, populated when extractable.
    # Balances are signed for posting use:
    #   - asset accounts (current/savings): positive when the customer holds money
    #   - liability accounts (credit card): negative when the customer owes money,
    #     positive when there is a credit balance
    period_start: date | None
    period_end: date | None
    opening_balance: Decimal | None
    closing_balance: Decimal | None


class BaseImporter(ABC):
    """Abstract base class for all importers."""

    name: str = ""
    # Short name for the format, shown on the import page, e.g. "HSBC PDF statements"
    label: str = ""
    description: str = ""
    file_types: list[str] = []

    @abstractmethod
    def parse(self, file_content: bytes) -> list[ImportedRow]:
        """Parse file content and return a list of imported rows.

        Args:
            file_content: Raw bytes of the uploaded file.

        Returns:
            List of ImportedRow objects, one per transaction.
        """

    def detect(self, file_content: bytes, filename: str = "") -> bool:
        """Check if this importer can handle the given file.

        Default implementation checks file extension against file_types.
        Override for smarter detection (e.g., header sniffing).
        """
        if not filename:
            return False
        ext = filename.rsplit(".", 1)[-1].lower() if "." in filename else ""
        return ext in self.file_types

    def detect_statement_info(self, file_content: bytes) -> StatementInfo | None:
        """Read the account and summary details from a statement, without importing it.

        Override for formats that identify their account. Returns None when the
        file carries no such details.
        """
        return None


def discover_importers() -> dict[str, BaseImporter]:
    """Discover all registered importers via entry points."""
    return load_plugins(ENTRY_POINT_GROUP)


def get_importer(name: str) -> BaseImporter | None:
    """Get a specific importer by name."""
    importers = discover_importers()
    return importers.get(name)


def find_importer(file_content: bytes, filename: str) -> BaseImporter | None:
    """The importer for a file: the first whose detect() recognises it, else the first
    that handles its file extension (so its parse() can say what's wrong with the file).

    The generic CSV importer is tried last, so a bank-specific importer that accepts
    CSV files wins over it.
    """
    importers = sorted(discover_importers().values(), key=lambda imp: imp.name == "csv")
    for importer in importers:
        if importer.detect(file_content, filename):
            return importer
    ext = filename.rsplit(".", 1)[-1].lower() if "." in filename else ""
    for importer in importers:
        if ext in importer.file_types:
            return importer
    return None
