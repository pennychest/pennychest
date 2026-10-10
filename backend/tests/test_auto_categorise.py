import httpx
import pytest

from pennychest.ai import providers
from pennychest.ai.config import set_provider_values
from pennychest.core.config import settings
from tests.conftest import use_model
from tests.test_ai import _expense_account, _source_id
from tests.test_tap_ai import FakeJev

STATEMENT = (
    b"Date,Description,Amount\n20/09/2026,OAKFIELD FARM SHOP,-8.40\n21/09/2026,TFL TRAVEL,-3.10\n"
)


@pytest.fixture(autouse=True)
def uploads(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "upload_dir", str(tmp_path))


@pytest.fixture
def jev(db_session, monkeypatch):
    fake = FakeJev()
    fake.answer("Expenses:Groceries", 0.9)
    monkeypatch.setattr(providers, "http_transport", httpx.MockTransport(fake.handler))
    set_provider_values(db_session, "typesafe", {"api_key": "ts-1"})
    use_model(db_session, "categorise", "typesafe", "jev-latest")
    return fake


def _accounts(client):
    card = client.post(
        "/api/accounts",
        json={"name": "Card", "full_path": "Liabilities:Card", "type": "liability"},
    ).json()
    for path in ("Expenses:Uncategorised", "Expenses:Groceries", "Expenses:Transport"):
        client.post(
            "/api/accounts",
            json={"name": path.split(":")[-1], "full_path": path, "type": "expense"},
        )
    return card["id"]


def _import(client, account_id, content=STATEMENT, name="september.csv"):
    response = client.post(
        "/api/imports/upload",
        files={"file": (name, content, "text/csv")},
        data={"account_id": str(account_id), "importer_name": "csv"},
    )
    assert response.status_code == 200, response.text
    return response.json()


def _batch_transactions(client, batch_id):
    txns = client.get("/api/transactions", params={"limit": 100}).json()
    txns = txns["transactions"] if isinstance(txns, dict) else txns
    found = [t for t in txns if t.get("import_batch_id") == batch_id]
    assert len(found) == 2
    return found


def test_imports_are_categorised_straight_away(client, jev):
    result = _import(client, _accounts(client))
    assert result["ai_categorised_count"] == 2
    assert result["uncategorised_count"] == 0
    assert result["ai_error"] is None
    for txn in _batch_transactions(client, result["batch_id"]):
        assert _expense_account(client, txn["id"]) == "Expenses:Groceries"

    [change] = client.get("/api/changes").json()["changes"]
    assert change["summary"] == "Categorised 2 transactions from september.csv"
    assert change["source"] == "import"


def test_the_import_can_be_undone_in_one_go(client, jev):
    result = _import(client, _accounts(client))
    [change] = client.get("/api/changes").json()["changes"]
    assert client.post(f"/api/changes/{change['id']}/undo").status_code == 200
    import_default = _source_id(client, "import_default")
    for txn in _batch_transactions(client, result["batch_id"]):
        detail = client.get(f"/api/transactions/{txn['id']}").json()
        expense = next(p for p in detail["postings"] if p["account_full_path"].startswith("Exp"))
        assert expense["account_full_path"] == "Expenses:Uncategorised"
        assert expense["categorised_by_id"] == import_default


def test_can_run_on_demand_instead(client, jev):
    assert client.get("/api/ai/config").json()["tasks"]["categorise"]["mode"] == "automatic"
    assert client.put("/api/ai/tasks/categorise", json={"mode": "on_demand"}).status_code == 204
    result = _import(client, _accounts(client))
    assert result["ai_categorised_count"] == 0 and result["uncategorised_count"] == 2
    assert jev.requests == []


def test_nothing_runs_without_a_model(client, jev, db_session):
    use_model(db_session, "categorise", None, None)
    result = _import(client, _accounts(client))
    assert result["ai_categorised_count"] == 0 and result["ai_error"] is None
    assert jev.requests == []


def test_a_failing_model_still_imports(client, jev):
    jev.status = 503
    result = _import(client, _accounts(client))
    assert result["transaction_count"] == 2 and result["uncategorised_count"] == 2
    assert result["ai_error"]
    assert client.get("/api/changes").json()["changes"] == []
    assert client.get("/api/ai/logs").json()["logs"][0]["error"]


def test_only_what_rules_left_is_sent(client, jev):
    account_id = _accounts(client)
    transport = next(
        a["id"]
        for a in client.get("/api/accounts").json()
        if a["full_path"] == "Expenses:Transport"
    )
    match_types = client.get("/api/lookup/match-types").json()
    client.post(
        "/api/rules",
        json={
            "pattern": "TFL",
            "match_type_id": next(m["id"] for m in match_types if m["name"] == "substring"),
            "target_account_id": transport,
            "priority": 10,
            "source": "manual",
            "description": "",
        },
    )
    result = _import(client, account_id)
    assert result["categorised_count"] == 1 and result["ai_categorised_count"] == 1
    assert list(jev.requests[-1]["state"].values()) == ["OAKFIELD FARM SHOP"]


def test_the_button_is_undoable_too(client, jev):
    client.put("/api/ai/tasks/categorise", json={"mode": "on_demand"})
    result = _import(client, _accounts(client))
    response = client.post(f"/api/ai/categorise/batch/{result['batch_id']}")
    assert response.json() == {"updated": 2}
    [change] = client.get("/api/changes").json()["changes"]
    assert change["source"] == "api"
