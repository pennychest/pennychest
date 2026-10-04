import sqlite3
from datetime import date
from decimal import Decimal

import pytest

from pennychest.core.database import engine
from pennychest.export import base, routes
from pennychest.export.base import BaseExporter, Ledger
from tests.test_ai import _source_id


class CsvExporter(BaseExporter):
    """Stands in for an exporter plugin, and keeps the ledger it was given."""

    name = "csv"
    label = "Spreadsheet"
    file_extension = "csv"
    media_type = "text/csv"
    received: Ledger | None = None

    def export(self, ledger: Ledger) -> str:
        CsvExporter.received = ledger
        return f"exported on {ledger.today.isoformat()}\n"


@pytest.fixture
def installed(monkeypatch):
    """Only the stand-in plugin is installed."""
    CsvExporter.received = None

    def discover():
        return {"csv": CsvExporter()}

    monkeypatch.setattr(base, "discover_exporters", discover)
    monkeypatch.setattr(routes, "discover_exporters", discover)


sqlite_only = pytest.mark.skipif(
    engine.url.get_backend_name() != "sqlite", reason="database backups are SQLite only"
)


@sqlite_only
def test_downloads_a_working_copy_of_the_database(client, tmp_path):
    info = client.get("/api/export").json()
    assert info["engine"] == "sqlite" and info["database_backup"] is True
    assert info["size_bytes"] > 0

    response = client.get("/api/export/database")
    assert response.status_code == 200
    disposition = response.headers["content-disposition"]
    assert "attachment" in disposition and "pennychest-backup-" in disposition
    copy = tmp_path / "backup.db"
    copy.write_bytes(response.content)
    tables = {r[0] for r in sqlite3.connect(copy).execute("select name from sqlite_master")}
    assert {"transactions", "postings", "accounts"} <= tables


@pytest.mark.skipif(engine.url.get_backend_name() == "sqlite", reason="Postgres only")
def test_postgres_explains_how_to_back_up(client):
    assert client.get("/api/export").json()["database_backup"] is False
    response = client.get("/api/export/database")
    assert response.status_code == 400 and "pg_dump" in response.json()["detail"]


def test_export_needs_a_session(anon_client, installed):
    assert anon_client.get("/api/export").status_code == 401
    assert anon_client.get("/api/export/database").status_code == 401
    assert anon_client.get("/api/export/csv").status_code == 401


def test_lists_installed_exporters(client, installed):
    [exporter] = client.get("/api/export").json()["exporters"]
    assert exporter == {
        "name": "csv",
        "label": "Spreadsheet",
        "description": "",
        "file_extension": "csv",
    }


def test_downloads_from_an_exporter_plugin(client, installed):
    response = client.get("/api/export/csv")
    assert response.status_code == 200
    assert response.headers["content-type"].startswith("text/csv")
    assert response.text.startswith("exported on ")
    assert f'pennychest-{date.today().isoformat()}.csv"' in response.headers["content-disposition"]


def test_unknown_exporter(client, installed):
    response = client.get("/api/export/nonesuch")
    assert response.status_code == 404 and "isn't installed" in response.json()["detail"]


def test_exporters_get_the_ledger_as_plain_data(client, installed):
    ids = {}
    for path, kind, ident in (
        ("Assets:Bank:Current:123456_12345678", "asset", "12-34-56 12345678"),
        ("Expenses:Food:Groceries", "expense", None),
    ):
        ids[path] = client.post(
            "/api/accounts",
            json={
                "name": path.split(":")[-1],
                "full_path": path,
                "type": kind,
                "bank_identifier": ident,
            },
        ).json()["id"]
    bank, food = ids["Assets:Bank:Current:123456_12345678"], ids["Expenses:Food:Groceries"]
    manual = _source_id(client, "manual")
    for day, description, amount, status in (
        ("2026-07-02", "COOP", "1.2345", "pending"),
        ("2026-07-01", "TESCO", "4.15", "confirmed"),
    ):
        client.post(
            "/api/transactions",
            json={
                "date": day,
                "description": description,
                "status": status,
                "postings": [
                    {"account_id": food, "amount": amount, "categorised_by_id": manual},
                    {"account_id": bank, "amount": f"-{amount}", "categorised_by_id": manual},
                ],
            },
        )
    client.post("/api/actions/set_budget", json={"category": "Groceries", "amount": "250"})

    assert client.get("/api/export/csv").status_code == 200
    ledger = CsvExporter.received
    assert ledger.today == date.today()

    accounts = {a.full_path: a for a in ledger.accounts}
    current = accounts["Assets:Bank:Current:123456_12345678"]
    assert (current.type, current.currency, current.bank_identifier) == (
        "asset",
        "GBP",
        "12-34-56 12345678",
    )
    assert accounts["Expenses:Food:Groceries"].type == "expense"

    # Oldest first, with exact amounts that balance
    tesco, coop = ledger.transactions
    assert (tesco.date, tesco.description, tesco.status) == (date(2026, 7, 1), "TESCO", "confirmed")
    assert coop.status == "pending"
    assert {(p.account_id, p.amount) for p in coop.postings} == {
        (food, Decimal("1.2345")),
        (bank, Decimal("-1.2345")),
    }

    [budget] = ledger.budgets
    assert (budget.account_id, budget.amount, budget.period) == (food, Decimal("250"), "monthly")
    assert budget.created_on == date.today()
