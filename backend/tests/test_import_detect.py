"""Choosing an importer for an uploaded file, and reading statement details through it."""

from datetime import date
from decimal import Decimal

import pytest

from pennychest.core.config import settings
from pennychest.imports import base, routes
from pennychest.imports.base import BaseImporter, ImportedRow, StatementInfo
from pennychest.imports.csv_importer import CsvImporter
from pennychest.imports.models import ImportBatch

CSV = b"Date,Description,Amount\n03/07/2026,TESCO,-4.00\n"


class FakeStatementImporter(BaseImporter):
    """Stands in for a plugin that reads statements identifying their account."""

    name = "fakebank"
    label = "Fake Bank statements"
    file_types = ["fake"]

    def detect(self, file_content: bytes, filename: str = "") -> bool:
        return file_content.startswith(b"FAKEBANK")

    def detect_statement_info(self, file_content: bytes) -> StatementInfo | None:
        return StatementInfo(
            statement_type="credit_card",
            bank_identifier="****4242",
            label="Fake Bank Credit Card Statement",
            name_hint="Fake Card",
            period_start=date(2026, 7, 1),
            period_end=date(2026, 7, 31),
            opening_balance=Decimal("-10.00"),
            closing_balance=Decimal("-14.00"),
        )

    def parse(self, file_content: bytes) -> list[ImportedRow]:
        return [ImportedRow(date(2026, 7, 3), "TESCO", Decimal("-4.00"), "TESCO")]


@pytest.fixture(autouse=True)
def uploads(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "upload_dir", str(tmp_path))


@pytest.fixture
def installed(monkeypatch):
    """Only the CSV importer and the fake plugin are installed."""

    def discover():
        return {"csv": CsvImporter(), "fakebank": FakeStatementImporter()}

    monkeypatch.setattr(base, "discover_importers", discover)
    monkeypatch.setattr(routes, "discover_importers", discover)


def _detect(client, filename, content):
    return client.post("/api/imports/detect", files={"file": (filename, content)})


def test_lists_installed_importers(client, installed):
    importers = {i["name"]: i for i in client.get("/api/imports/importers").json()}
    assert importers["fakebank"]["label"] == "Fake Bank statements"
    assert importers["csv"]["file_types"] == ["csv", "txt"]


def test_csv_needs_no_statement_details(client, installed):
    response = _detect(client, "july.csv", CSV)
    assert response.status_code == 200, response.text
    assert response.json() == {"importer": "csv", "statement": None}


def test_plugin_statement_suggests_its_account(client, installed):
    card = client.post(
        "/api/accounts",
        json={
            "name": "Fake Card",
            "full_path": "Liabilities:CreditCard:Fake",
            "type": "liability",
            "bank_identifier": "****4242",
        },
    ).json()

    response = _detect(client, "july.fake", b"FAKEBANK statement")
    assert response.status_code == 200, response.text
    body = response.json()
    assert body["importer"] == "fakebank"
    statement = body["statement"]
    assert statement["suggested_account_id"] == card["id"]
    assert statement["period_start"] == "2026-07-01"
    assert statement["opening_balance"] == "-10.00"


def test_unrecognised_file_goes_to_the_importer_for_its_extension(client, installed):
    # So the importer's parse() can explain what's wrong with it on upload
    response = _detect(client, "other.fake", b"not a statement")
    assert response.json()["importer"] == "fakebank"


def test_unsupported_file_type(client, installed):
    response = _detect(client, "photo.jpg", b"\xff\xd8")
    assert response.status_code == 422
    assert ".csv" in response.json()["detail"] and ".fake" in response.json()["detail"]


def test_upload_records_the_statement_summary(client, db_session, installed):
    card = client.post(
        "/api/accounts",
        json={"name": "Fake Card", "full_path": "Liabilities:CreditCard:Fake", "type": "liability"},
    ).json()
    client.post(
        "/api/accounts",
        json={"name": "Uncategorised", "full_path": "Expenses:Uncategorised", "type": "expense"},
    )
    response = client.post(
        "/api/imports/upload",
        files={"file": ("july.fake", b"FAKEBANK statement")},
        data={"account_id": str(card["id"]), "importer_name": "fakebank"},
    )
    assert response.status_code == 200, response.text

    batch = db_session.get(ImportBatch, response.json()["batch_id"])
    assert (batch.period_start, batch.period_end) == (date(2026, 7, 1), date(2026, 7, 31))
    assert batch.closing_balance == Decimal("-14.00")


def test_upload_with_an_importer_that_isnt_installed(client, installed):
    card = client.post(
        "/api/accounts",
        json={"name": "Card", "full_path": "Liabilities:CreditCard:Card", "type": "liability"},
    ).json()
    response = client.post(
        "/api/imports/upload",
        files={"file": ("july.pdf", b"%PDF")},
        data={"account_id": str(card["id"]), "importer_name": "hsbc"},
    )
    assert response.status_code == 400
    assert "isn't installed" in response.json()["detail"]
