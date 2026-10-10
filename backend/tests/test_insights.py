import json
from contextlib import nullcontext

import httpx
import pytest

from pennychest.ai import insights, providers
from pennychest.ai.config import set_provider_values
from pennychest.ai.providers import Call
from pennychest.core.config import settings
from tests.conftest import use_model


class FakeJev:
    """Answers Choice questions with `kind(merchant)` and Score questions with
    `score(context)`, or fails with `status`."""

    def __init__(self):
        self.kind = lambda merchant: ("other", 0.9)
        self.score = lambda context: 0.0
        self.status = 200
        self.requests: list[dict] = []

    def handler(self, request: httpx.Request) -> httpx.Response:
        body = json.loads(request.content)
        self.requests.append(body)
        if self.status != 200:
            return httpx.Response(self.status, json={"error": "overloaded"})
        answers = {}
        for key, question in body["questions"].items():
            item = body["state"][key]
            if question["type"] == "choice":
                choice, confidence = self.kind(item)
                answers[key] = {"type": "choice", "choice": choice, "confidence": confidence}
            else:
                assert question["type"] == "score" and len(question["criteria"]) == 4
                answers[key] = {"type": "score", "score": self.score(item), "confidence": 0.8}
        return httpx.Response(200, json={"model": "jev-1", "answers": answers})

    def asked(self, kind):
        return [
            body for body in self.requests
            if next(iter(body["questions"].values()))["type"] == kind
        ]


@pytest.fixture(autouse=True)
def uploads(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "upload_dir", str(tmp_path))


@pytest.fixture(autouse=True)
def same_session(db_session, monkeypatch):
    """The background job opens its own session; in tests it must share the test's."""
    monkeypatch.setattr(insights, "SessionLocal", lambda: nullcontext(db_session))


@pytest.fixture
def jev(db_session, monkeypatch):
    fake = FakeJev()
    fake.kind = lambda m: ("subscription", 0.95) if "NETFLIX" in m["merchant"] else ("other", 0.9)
    fake.score = lambda c: 3.0 if float(c["charge"]["amount"]) > 500 else 0.2
    monkeypatch.setattr(providers, "http_transport", httpx.MockTransport(fake.handler))
    set_provider_values(db_session, "typesafe", {"api_key": "ts-1"})
    use_model(db_session, "insights", "typesafe", "jev-latest")
    return fake


@pytest.fixture
def current(client):
    for path, type_ in [("Assets:Current", "asset"), ("Expenses:Uncategorised", "expense")]:
        client.post(
            "/api/accounts", json={"name": path.split(":")[-1], "full_path": path, "type": type_}
        )
    accounts = client.get("/api/accounts").json()
    return next(a["id"] for a in accounts if a["full_path"] == "Assets:Current")


HISTORY = [
    ("15/05/2026", "NETFLIX.COM 0588", "-10.99"),
    ("15/06/2026", "NETFLIX.COM 0688", "-10.99"),
    ("15/07/2026", "NETFLIX.COM 0788", "-10.99"),
    ("15/08/2026", "NETFLIX.COM 0888", "-10.99"),
    ("27/08/2026", "TESCO STORES", "-40.00"),
    ("03/09/2026", "TESCO STORES", "-41.00"),
    ("10/09/2026", "TESCO STORES", "-39.00"),
]
SEPTEMBER = [
    ("15/09/2026", "NETFLIX.COM 0988", "-12.99"),
    ("17/09/2026", "TESCO STORES", "-40.50"),
    ("21/09/2026", "ELECTRONICS WORLD", "-899.00"),
    ("22/09/2026", "REFUND ACME", "15.00"),
]


def _import(client, account_id, rows, name):
    content = "Date,Description,Amount\n" + "".join(f"{d},{desc},{amt}\n" for d, desc, amt in rows)
    response = client.post(
        "/api/imports/upload",
        files={"file": (name, content.encode(), "text/csv")},
        data={"account_id": str(account_id), "importer_name": "csv"},
    )
    assert response.status_code == 200, response.text
    batch_id = response.json()["batch_id"]
    return {
        t["description"]: t
        for t in client.get(f"/api/imports/batches/{batch_id}").json()["transactions"]
    }


def _action(client, name, **arguments):
    response = client.post(f"/api/actions/{name}", json=arguments)
    assert response.status_code == 200, response.text
    return response.json()


def test_imports_are_scored_against_each_merchants_history(client, current, jev):
    _import(client, current, HISTORY, "history.csv")
    jev.requests.clear()
    september = _import(client, current, SEPTEMBER, "september.csv")

    assert september["ELECTRONICS WORLD"]["unusual_score"] == 1.0
    assert september["TESCO STORES"]["unusual_score"] == pytest.approx(0.067, abs=0.001)
    assert september["REFUND ACME"]["unusual_score"] is None  # refunds aren't charges

    [scored] = jev.asked("score")
    contexts = {c["charge"]["description"]: c for c in scored["state"].values()}
    assert set(contexts) == {"NETFLIX.COM 0988", "TESCO STORES", "ELECTRONICS WORLD"}
    assert [h["amount"] for h in contexts["TESCO STORES"]["merchant_history"]] == [
        "40.00",
        "41.00",
        "39.00",
    ]
    assert contexts["ELECTRONICS WORLD"]["merchant_history"] == []
    assert "category_last_year" not in contexts["TESCO STORES"]  # uncategorised


def test_recurring_merchants_are_labelled_once(client, current, jev):
    _import(client, current, HISTORY, "history.csv")
    _import(client, current, SEPTEMBER, "september.csv")

    # Tesco is weekly, so it only still counts as active within ten days or so of its last
    payments = _action(client, "recurring_payments", on="2026-09-20")["payments"]
    assert {(p["merchant"], p["kind"]) for p in payments} == {
        ("NETFLIX.COM 0988", "subscription"),
        ("TESCO STORES", "other"),
    }
    only = _action(client, "recurring_payments", on="2026-09-20", kind="subscription")
    assert [p["merchant"] for p in only["payments"]] == ["NETFLIX.COM 0988"]
    assert only["yearly_total"] == "131.88"  # Tesco left out of the total too

    labelled = [m["merchant"] for body in jev.asked("choice") for m in body["state"].values()]
    _import(client, current, [("24/09/2026", "TESCO STORES", "-38.00")], "more.csv")
    relabelled = [m["merchant"] for body in jev.asked("choice") for m in body["state"].values()]
    assert relabelled == labelled  # nothing new to label


def test_unusual_charges_and_price_rises(client, current, jev):
    _import(client, current, HISTORY, "history.csv")
    _import(client, current, SEPTEMBER, "september.csv")
    result = _action(
        client, "unusual_charges", start_date="2026-09-01", end_date="2026-09-30"
    )
    found = [(c["description"], c["amount"], c["unusual_score"]) for c in result["unusual_charges"]]
    assert found == [("ELECTRONICS WORLD", "899.00", 1.0)]
    assert result["price_rises"] == [
        {
            "merchant": "NETFLIX.COM 0988",
            "kind": "subscription",
            "cadence": "monthly",
            "usual_amount": "10.99",
            "latest_amount": "12.99",
            "latest_date": "2026-09-15",
            "increase": "2.00",
        }
    ]
    # Tesco's charges come from the history import, which was scored when it arrived
    assert result["unscored_charges"] == 0


def test_nothing_is_judged_unless_turned_on(client, current, monkeypatch):
    fake = FakeJev()
    monkeypatch.setattr(providers, "http_transport", httpx.MockTransport(fake.handler))
    september = _import(client, current, SEPTEMBER, "september.csv")
    assert fake.requests == []
    assert september["ELECTRONICS WORLD"]["unusual_score"] is None
    result = _action(client, "unusual_charges", start_date="2026-09-01", end_date="2026-09-30")
    assert result["unusual_charges"] == [] and result["unscored_charges"] == 3


def test_a_failing_model_never_breaks_the_import(client, current, jev):
    jev.status = 503
    september = _import(client, current, SEPTEMBER, "september.csv")
    assert september["ELECTRONICS WORLD"]["unusual_score"] is None
    logs = client.get("/api/ai/logs").json()["logs"]
    assert any(log["operation"] == "score_charges" and log["error"] for log in logs)


def test_llms_score_and_label_with_json(client, current, db_session, monkeypatch):
    history = _import(client, current, HISTORY, "history.csv")

    class FakeLLM(providers.Provider):
        id, label = "fake", "Fake"

        def complete_json(self, cfg, model, system, prompt, schema, name):
            listing = json.loads(prompt[prompt.index("{") if name == "score_charges"
                                        else prompt.index("[\n"):])
            if name == "score_charges":
                return Call({"scores": [{"id": int(i), "level": 2} for i in listing]}, {}, {})
            return Call({"labels": [{"index": i, "kind": "bill"} for i, _ in listing]}, {}, {})

    monkeypatch.setattr(insights, "resolve_task", lambda db, task: (FakeLLM(), {}, "m1"))
    use_model(db_session, "insights", "openai", "gpt")
    batch_id = history["TESCO STORES"]["import_batch_id"]
    assert insights.run_after_import(db_session, batch_id) is None
    payments = _action(client, "recurring_payments", on="2026-09-10")["payments"]
    assert {p["kind"] for p in payments} == {"bill"}
    unusual = _action(client, "unusual_charges", start_date="2026-05-01", end_date="2026-09-30")
    assert {c["unusual_score"] for c in unusual["unusual_charges"]} == {0.667}


def test_on_demand_insights_run_from_the_review_page(client, current, jev, db_session):
    use_model(db_session, "insights", "typesafe", "jev-latest", mode="on_demand")
    _import(client, current, HISTORY, "history.csv")
    assert jev.requests == []

    batch_id = max(b["id"] for b in client.get("/api/imports/batches").json())
    assert client.post(f"/api/ai/insights/batch/{batch_id}").status_code == 200
    assert jev.requests
