import httpx
import pytest

from pennychest.ai import learned, providers
from pennychest.ai.config import set_provider_values, set_task
from tests.test_ai import _expense_account, _source_id
from tests.test_auto_categorise import _import, uploads  # noqa: F401
from tests.test_tap_ai import FakeJev, same_session  # noqa: F401
from tests.test_taps import _tap, api_token  # noqa: F401

HISTORY = (
    [(f"TESCO STORES {n} ST HELIER", "Expenses:Groceries") for n in range(12)]
    + [(f"SOJ PARKING JERSEY {n}", "Expenses:Transport") for n in range(12)]
    + [("NETFLIX.COM LONDON", "Expenses:Entertainment")] * 8
)


def setup_accounts(client):
    accounts = {}
    for path, kind in (
        ("Liabilities:Card", "liability"),
        ("Expenses:Uncategorised", "expense"),
        ("Expenses:Groceries", "expense"),
        ("Expenses:Transport", "expense"),
        ("Expenses:Entertainment", "expense"),
    ):
        accounts[path] = client.post(
            "/api/accounts", json={"name": path.split(":")[-1], "full_path": path, "type": kind}
        ).json()["id"]
    return accounts


def add(client, accounts, description, category, source="manual", status=None):
    body = {
        "date": "2026-06-01",
        "description": description,
        "postings": [
            {
                "account_id": accounts["Liabilities:Card"],
                "amount": "-5.00",
                "categorised_by_id": _source_id(client, source),
            },
            {
                "account_id": accounts[category],
                "amount": "5.00",
                "categorised_by_id": _source_id(client, source),
            },
        ],
    }
    if status:
        body["status"] = status
    return client.post("/api/transactions", json=body).json()["id"]


@pytest.fixture
def history(client):
    accounts = setup_accounts(client)
    for description, category in HISTORY:
        add(client, accounts, description, category)
    return accounts


def predict(db_session, *descriptions):
    return learned.suggest(db_session, list(descriptions))


def test_needs_enough_history(client, db_session):
    accounts = setup_accounts(client)
    for description, category in HISTORY[:10]:
        add(client, accounts, description, category)
    assert predict(db_session, "TESCO STORES 99") == [None]
    status = client.get("/api/ai/learned").json()
    assert status["ready"] is False and status["examples"] == 10


def test_learns_merchants_and_stays_quiet_when_unsure(db_session, history):
    groceries, unsure = predict(db_session, "TESCO STORES 4471 ST BRELADE", "PAYPAL PAYMENT")
    assert groceries[0] == history["Expenses:Groceries"] and groceries[1] >= 0.7
    assert unsure is None


def test_learns_only_what_the_user_stands_behind(client, db_session):
    accounts = setup_accounts(client)
    for description, category in HISTORY:
        add(client, accounts, description, category, source="ai")  # pending AI guesses
    assert client.get("/api/ai/learned").json()["examples"] == 0
    assert predict(db_session, "TESCO STORES 1") == [None]


def test_retrains_when_history_changes(client, db_session, history):
    assert predict(db_session, "ODEON CINEMA")[0] is None
    for n in range(10):
        add(client, history, f"ODEON CINEMA {n}", "Expenses:Entertainment")
    assert predict(db_session, "ODEON CINEMA")[0][0] == history["Expenses:Entertainment"]


def test_status_reports_a_check_on_past_data(client, history):
    status = client.get("/api/ai/learned").json()
    assert status["ready"] is True
    assert status["examples"] == len(HISTORY) and status["categories"] == 3
    assert 0 < status["check"]["coverage"] <= 1
    assert status["check"]["accuracy"] > 0.9


def test_can_be_turned_off(client, db_session, history):
    assert client.put("/api/ai/learned", json={"enabled": False}).status_code == 204
    assert client.get("/api/ai/learned").json()["enabled"] is False
    assert predict(db_session, "TESCO STORES 1") == [None]


STATEMENT = (
    b"Date,Description,Amount\n"
    b"01/07/2026,TESCO STORES 9001 ST HELIER,-12.00\n"
    b"02/07/2026,SOJ PARKING JERSEY JEY,-3.30\n"
    b"03/07/2026,OAKFIELD FARM SHOP,-8.40\n"
)


def test_imports_use_history_without_any_ai(client, history):
    result = _import(client, history["Liabilities:Card"], STATEMENT, name="july.csv")
    assert result["learned_count"] == 2 and result["ai_error"] is None
    assert result["uncategorised_count"] == 1

    txns = client.get("/api/transactions", params={"per_page": 100}).json()
    by_description = {t["description"]: t["id"] for t in txns}
    tesco = by_description["TESCO STORES 9001 ST HELIER"]
    assert _expense_account(client, tesco) == "Expenses:Groceries"
    detail = client.get(f"/api/transactions/{tesco}").json()
    expense = next(p for p in detail["postings"] if p["account_full_path"].startswith("Exp"))
    assert expense["categorised_by_id"] == _source_id(client, "learned")

    [change] = client.get("/api/changes").json()["changes"]
    assert change["summary"] == "Categorised 2 transactions from july.csv (2 from your history)"
    assert client.post(f"/api/changes/{change['id']}/undo").status_code == 200
    assert _expense_account(client, tesco) == "Expenses:Uncategorised"


def test_only_what_history_cant_answer_goes_to_the_ai(client, db_session, monkeypatch, history):
    fake = FakeJev()
    fake.answer("Expenses:Groceries", 0.9)
    monkeypatch.setattr(providers, "http_transport", httpx.MockTransport(fake.handler))
    set_provider_values(db_session, "typesafe", {"api_key": "ts-1"})
    set_task(db_session, "categorise", "typesafe", "jev-latest")

    result = _import(client, history["Liabilities:Card"], STATEMENT, name="july.csv")
    assert result["learned_count"] == 2 and result["ai_categorised_count"] == 3
    [sent] = fake.requests
    assert list(sent["state"].values()) == ["OAKFIELD FARM SHOP"]


def test_taps_use_history_before_the_ai(client, db_session, monkeypatch, history):
    fake = FakeJev()
    monkeypatch.setattr(providers, "http_transport", httpx.MockTransport(fake.handler))
    set_provider_values(db_session, "typesafe", {"api_key": "ts-1"})
    set_task(db_session, "taps", "typesafe", "jev-latest")

    tap = _tap(client, merchant="TESCO STORES 3301 ST HELIER").json()
    assert tap["suggestion_source"] == "learned"
    assert tap["suggested_account_full_path"] == "Expenses:Groceries"
    assert tap["suggestion_confidence"] >= 0.7
    assert fake.requests == []
