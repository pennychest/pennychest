import json
from contextlib import nullcontext

import httpx
import pytest

from pennychest.ai import providers
from pennychest.ai.config import set_provider_values
from pennychest.taps import ai as tap_ai
from tests.conftest import use_model
from tests.test_ai import _expense_account, _imported_transactions, _source_id
from tests.test_taps import _tap, api_token  # noqa: F401


@pytest.fixture(autouse=True)
def same_session(db_session, monkeypatch):
    """The background job opens its own session; in tests it must share the test's."""
    monkeypatch.setattr(tap_ai, "SessionLocal", lambda: nullcontext(db_session))


class FakeJev:
    """Answers every Choice question with `choice` at `confidence`, or fails with `status`."""

    def __init__(self):
        self.choice, self.confidence, self.status = None, 0.0, 200
        self.requests: list[dict] = []

    def handler(self, request: httpx.Request) -> httpx.Response:
        body = json.loads(request.content)
        self.requests.append(body)
        if self.status != 200:
            return httpx.Response(self.status, json={"error": "overloaded"})
        answers = {
            key: {"type": "choice", "choice": self.choice, "confidence": self.confidence}
            for key in body["questions"]
        }
        return httpx.Response(200, json={"model": "jev-1", "answers": answers})

    def answer(self, choice, confidence):
        self.choice, self.confidence = choice, confidence


@pytest.fixture
def jev(db_session, monkeypatch):
    fake = FakeJev()
    monkeypatch.setattr(providers, "http_transport", httpx.MockTransport(fake.handler))
    set_provider_values(db_session, "typesafe", {"api_key": "ts-1"})
    use_model(db_session, "taps", "typesafe", "jev-latest")
    return fake


@pytest.fixture
def jev_off(monkeypatch):
    fake = FakeJev()
    monkeypatch.setattr(providers, "http_transport", httpx.MockTransport(fake.handler))
    return fake


def _categories(client):
    for path in ("Expenses:Uncategorised", "Expenses:Groceries", "Expenses:Transport"):
        client.post(
            "/api/accounts",
            json={"name": path.split(":")[-1], "full_path": path, "type": "expense"},
        )


def test_tap_is_categorised_after_the_reply(client, jev):
    _categories(client)
    jev.answer("Expenses:Groceries", 0.92)
    reply = _tap(client)
    assert reply.status_code == 201
    # The reply to the phone doesn't wait for the model
    assert reply.json()["suggested_account_full_path"] is None

    [tap] = client.get("/api/taps").json()
    assert tap["suggested_account_full_path"] == "Expenses:Groceries"
    assert tap["suggestion_source"] == "ai"
    assert tap["suggestion_confidence"] == pytest.approx(0.92)

    sent = jev.requests[-1]
    assert sent["model"] == "jev-latest"
    assert list(sent["state"].values()) == ["Tesco (GBP 12.50)"]
    criteria = next(iter(sent["questions"].values()))["criteria"]
    assert "Expenses:Uncategorised" not in criteria and "Expenses:Groceries" in criteria

    log = client.get("/api/ai/logs").json()["logs"][0]
    assert log["operation"] == "tap_categorise" and log["error"] is None


def test_low_confidence_answers_are_left_for_the_user(client, jev):
    _categories(client)
    jev.answer("Expenses:Transport", 0.2)
    _tap(client)
    assert client.get("/api/taps").json()[0]["suggested_account_full_path"] is None


def test_nothing_runs_unless_turned_on(client, jev_off):
    _categories(client)
    _tap(client)
    assert jev_off.requests == []


def test_rules_win_and_skip_the_model(client, jev):
    _categories(client)
    groceries = next(
        a["id"]
        for a in client.get("/api/accounts").json()
        if a["full_path"] == "Expenses:Groceries"
    )
    match_types = client.get("/api/lookup/match-types").json()
    substring = next(m["id"] for m in match_types if m["name"] == "substring")
    client.post(
        "/api/rules",
        json={
            "pattern": "tesco",
            "match_type_id": substring,
            "target_account_id": groceries,
            "priority": 0,
            "source": "manual",
            "description": "",
        },
    )
    assert _tap(client).json()["suggestion_source"] == "rule"
    assert jev.requests == []


def test_a_failing_model_never_breaks_the_tap(client, jev):
    _categories(client)
    jev.status = 503
    reply = _tap(client)
    assert reply.status_code == 201
    assert client.get("/api/taps").json()[0]["suggested_account_full_path"] is None
    assert client.get("/api/ai/logs").json()["logs"][0]["error"]


def test_statement_import_takes_the_taps_category(client, jev):
    jev.answer("Expenses:Groceries", 0.9)
    (txn_id,), accounts = _imported_transactions(client, "TESCO STORES 2041")
    _tap(client, amount="£5.00")
    client.put(
        "/api/taps/cards", json={"card_name": "My Visa", "account_id": accounts["Liabilities:Card"]}
    )
    assert client.get("/api/taps?status=reconciled").json()[0]["transaction_id"] == txn_id
    assert _expense_account(client, txn_id) == "Expenses:Groceries"
    txn = client.get(f"/api/transactions/{txn_id}").json()
    expense = next(p for p in txn["postings"] if p["account_full_path"].startswith("Expenses"))
    assert expense["categorised_by_id"] == _source_id(client, "ai")


def test_statement_categories_are_never_overwritten(client, jev):
    jev.answer("Expenses:Transport", 0.9)
    _, accounts = _imported_transactions(client)
    # A statement line the import (or the user) already categorised
    manual = _source_id(client, "manual")
    txn_id = client.post(
        "/api/transactions",
        json={
            "date": "2026-09-20",
            "description": "TESCO STORES 2041",
            "postings": [
                {
                    "account_id": accounts["Liabilities:Card"],
                    "amount": "-5.00",
                    "categorised_by_id": manual,
                },
                {
                    "account_id": accounts["Expenses:Groceries"],
                    "amount": "5.00",
                    "categorised_by_id": manual,
                },
            ],
        },
    ).json()["id"]
    _tap(client, amount="£5.00")
    client.put(
        "/api/taps/cards", json={"card_name": "My Visa", "account_id": accounts["Liabilities:Card"]}
    )
    assert client.get("/api/taps?status=reconciled").json()[0]["transaction_id"] == txn_id
    assert _expense_account(client, txn_id) == "Expenses:Groceries"


def test_tasks_that_can_run_by_themselves(client):
    tasks = client.get("/api/ai/config").json()["tasks"]
    assert {name for name, t in tasks.items() if t["mode"] is not None} == {
        "taps",
        "categorise",
        "matching",
        "insights",
        "csv_columns",
    }


def test_on_demand_taps_wait_to_be_asked(client, jev, db_session):
    use_model(db_session, "taps", "typesafe", "jev-latest", mode="on_demand")
    _categories(client)
    jev.answer("Expenses:Groceries", 0.92)
    _tap(client)
    assert jev.requests == []

    [tap] = client.get("/api/taps").json()
    response = client.post(f"/api/taps/{tap['id']}/categorise")
    assert response.status_code == 200
    assert response.json()["suggested_account_full_path"] == "Expenses:Groceries"


def test_asking_for_a_tap_category_needs_the_task_on(client, jev_off):
    _categories(client)
    _tap(client)
    [tap] = client.get("/api/taps").json()
    response = client.post(f"/api/taps/{tap['id']}/categorise")
    assert response.status_code == 400
    assert "turned off" in response.json()["detail"]
