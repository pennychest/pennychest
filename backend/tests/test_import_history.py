from datetime import date

import pytest

from pennychest.core.config import settings
from pennychest.imports.models import ImportBatch

STATEMENT = b"Date,Description,Amount\n03/07/2026,TESCO,-4.00\n"


@pytest.fixture(autouse=True)
def uploads(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "upload_dir", str(tmp_path))


def test_batches_name_their_account_and_period(client, db_session):
    account = client.post(
        "/api/accounts",
        json={
            "name": "HSBC Current",
            "full_path": "Assets:Bank:Current:654321_87654321",
            "type": "asset",
            "bank_identifier": "654321 87654321",
        },
    ).json()
    client.post(
        "/api/accounts",
        json={"name": "Uncategorised", "full_path": "Expenses:Uncategorised", "type": "expense"},
    )
    response = client.post(
        "/api/imports/upload",
        files={"file": ("july.csv", STATEMENT, "text/csv")},
        data={"account_id": str(account["id"]), "importer_name": "csv"},
    )
    assert response.status_code == 200, response.text

    [batch] = client.get("/api/imports/batches").json()
    assert batch["account_name"] == "HSBC Current"
    assert batch["account_number_hint"] == "4321"
    assert batch["period_start"] is None and batch["file_name"] == "july.csv"

    # Statements that state their period (HSBC PDFs) report it
    stored = db_session.get(ImportBatch, batch["id"])
    stored.period_start, stored.period_end = date(2026, 7, 1), date(2026, 7, 31)
    db_session.commit()
    [batch] = client.get("/api/imports/batches").json()
    assert (batch["period_start"], batch["period_end"]) == ("2026-07-01", "2026-07-31")
