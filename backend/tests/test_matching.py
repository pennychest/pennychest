import json
from contextlib import nullcontext

import httpx
import pytest

from pennychest.ai import providers
from pennychest.ai.config import set_provider_values
from pennychest.core.config import settings
from pennychest.taps import service as tap_service
from tests.conftest import use_model
from tests.test_taps import AUTH, _setup, _statement_line, api_token  # noqa: F401


class FakeJev:
    """Answers each Noul question with `judge(a, b)`, or fails with `status`."""

    def __init__(self, judge=lambda a, b: 0.0):
        self.judge, self.status = judge, 200
        self.requests: list[dict] = []

    def handler(self, request: httpx.Request) -> httpx.Response:
        body = json.loads(request.content)
        self.requests.append(body)
        if self.status != 200:
            return httpx.Response(self.status, json={"error": "overloaded"})
        answers = {}
        for key, question in body["questions"].items():
            assert question["type"] == "noul" and f"`{key}.a`" in question["instructions"]
            pair = body["state"][key]
            answers[key] = {"type": "noul", "noul": self.judge(pair["a"], pair["b"])}
        return httpx.Response(200, json={"model": "jev-1", "answers": answers})


@pytest.fixture(autouse=True)
def uploads(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "upload_dir", str(tmp_path))


@pytest.fixture
def jev(db_session, monkeypatch):
    fake = FakeJev()
    monkeypatch.setattr(providers, "http_transport", httpx.MockTransport(fake.handler))
    set_provider_values(db_session, "typesafe", {"api_key": "ts-1"})
    use_model(db_session, "matching", "typesafe", "jev-latest")
    return fake


@pytest.fixture
def jev_off(monkeypatch):
    fake = FakeJev(lambda a, b: 1.0)
    monkeypatch.setattr(providers, "http_transport", httpx.MockTransport(fake.handler))
    return fake


def _account(client, path, type_):
    return client.post(
        "/api/accounts", json={"name": path.split(":")[-1], "full_path": path, "type": type_}
    ).json()["id"]


@pytest.fixture
def accounts(client):
    return {
        path: _account(client, path, type_)
        for path, type_ in [
            ("Assets:Current", "asset"),
            ("Liabilities:Card", "liability"),
            ("Assets:Transfers:Pending", "asset"),
            ("Expenses:Uncategorised", "expense"),
        ]
    }


def _import(client, account_id, rows, name):
    content = "Date,Description,Amount\n" + "".join(f"{d},{desc},{amt}\n" for d, desc, amt in rows)
    response = client.post(
        "/api/imports/upload",
        files={"file": (name, content.encode(), "text/csv")},
        data={"account_id": str(account_id), "importer_name": "csv"},
    )
    assert response.status_code == 200, response.text
    batch_id = response.json()["batch_id"]
    return client.get(f"/api/imports/batches/{batch_id}").json()["transactions"]


def _by_description(transactions):
    return {t["description"]: t for t in transactions}


# Duplicates


def test_exact_duplicates_are_flagged_without_a_model(client, accounts, jev_off):
    current = accounts["Assets:Current"]
    first = _import(client, current, [("20/09/2026", "TESCO STORES", "-12.50")], "a.csv")
    second = _import(
        client,
        current,
        [("20/09/2026", "Tesco Stores", "-12.50"), ("21/09/2026", "TFL", "-3.10")],
        "b.csv",
    )
    flagged = _by_description(second)
    assert flagged["Tesco Stores"]["duplicate_of"] == {
        "transaction_id": first[0]["id"],
        "date": "2026-09-20",
        "description": "TESCO STORES",
        "source": "imported from a.csv",
    }
    assert flagged["TFL"]["duplicate_of"] is None
    assert first[0]["duplicate_of"] is None
    assert jev_off.requests == []


def test_the_model_spots_duplicates_described_differently(client, accounts, jev):
    jev.judge = lambda a, b: 0.95 if "TESCO" in a["description"].upper() else 0.1
    current = accounts["Assets:Current"]
    first = _import(
        client,
        current,
        [("20/09/2026", "TESCO STORES 2231", "-12.50"), ("20/09/2026", "PRET", "-4.00")],
        "a.csv",
    )
    second = _import(
        client,
        current,
        [("22/09/2026", "Tesco Stores London", "-12.50"), ("21/09/2026", "Pret A Manger", "-4.00")],
        "b.pdf.csv",
    )
    found = _by_description(second)
    tesco = _by_description(first)["TESCO STORES 2231"]
    assert found["Tesco Stores London"]["duplicate_of"]["transaction_id"] == tesco["id"]
    assert found["Pret A Manger"]["duplicate_of"] is None
    [request] = jev.requests
    assert request["state"]["p0"]["b"]["source"] == "imported from a.csv"


def test_same_day_repeats_within_one_statement_are_not_duplicates(client, accounts, jev_off):
    rows = [("20/09/2026", "COFFEE", "-3.00"), ("20/09/2026", "COFFEE", "-3.00")]
    imported = _import(client, accounts["Assets:Current"], rows, "a.csv")
    assert [t["duplicate_of"] for t in imported] == [None, None]


def test_flagged_duplicates_can_be_kept_or_deleted(client, accounts, jev_off):
    current = accounts["Assets:Current"]
    _import(client, current, [("20/09/2026", "TESCO", "-12.50")], "a.csv")
    rows = [("20/09/2026", "TESCO", "-12.50"), ("21/09/2026", "TFL", "-3.10")]
    _import(client, current, rows, "b.csv")
    second = _import(client, current, rows + [("22/09/2026", "BOOTS", "-6.00")], "c.csv")
    found = _by_description(second)
    tesco, tfl = found["TESCO"], found["TFL"]
    assert tesco["duplicate_of"] and tfl["duplicate_of"] and not found["BOOTS"]["duplicate_of"]

    assert client.post(f"/api/imports/transactions/{tesco['id']}/not-duplicate").status_code == 204
    assert client.delete(f"/api/transactions/{tfl['id']}").status_code == 204
    batch_id = tesco["import_batch_id"]
    remaining = client.get(f"/api/imports/batches/{batch_id}").json()["transactions"]
    assert {t["description"]: t["duplicate_of"] for t in remaining} == {
        "TESCO": None,
        "BOOTS": None,
    }


# Transfers


def test_confident_transfers_are_linked(client, accounts, jev):
    jev.judge = lambda a, b: 0.9 if "AMEX" in a["description"] + b["description"] else 0.2
    card = _import(
        client,
        accounts["Liabilities:Card"],
        [("21/09/2026", "PAYMENT RECEIVED - THANK YOU", "250.00")],
        "card.csv",
    )
    current = _import(
        client,
        accounts["Assets:Current"],
        [("20/09/2026", "AMEX DD 0042", "-250.00"), ("20/09/2026", "RENT", "-250.00")],
        "current.csv",
    )
    found = _by_description(current)
    assert found["AMEX DD 0042"]["transfer_peer_id"] == card[0]["id"]
    assert found["AMEX DD 0042"]["target_account_full_path"] == "Assets:Transfers:Pending"
    assert found["AMEX DD 0042"]["categorised_by"] == "transfer"
    assert found["RENT"]["transfer_peer_id"] is None
    assert found["RENT"]["target_account_full_path"] == "Expenses:Uncategorised"
    card_side = client.get(f"/api/imports/batches/{card[0]['import_batch_id']}").json()
    assert card_side["transactions"][0]["transfer_peer_id"] == found["AMEX DD 0042"]["id"]
    assert card_side["transactions"][0]["target_account_full_path"] == "Assets:Transfers:Pending"

    sent = jev.requests[-1]["state"]["p0"]
    assert sent["a"]["account"] == "Assets:Current" and sent["b"]["account"] == "Liabilities:Card"
    assert sent["a"]["amount"].startswith("-250") and sent["b"]["amount"].startswith("250")


def test_unsure_transfers_are_left_alone(client, accounts, jev):
    jev.judge = lambda a, b: 0.6
    _import(client, accounts["Liabilities:Card"], [("21/09/2026", "PAYMENT", "250.00")], "c.csv")
    current = _import(
        client, accounts["Assets:Current"], [("20/09/2026", "AMEX", "-250.00")], "d.csv"
    )
    assert current[0]["transfer_peer_id"] is None
    assert current[0]["target_account_full_path"] == "Expenses:Uncategorised"


def test_transfers_need_the_model_turned_on(client, accounts, jev_off):
    _import(client, accounts["Liabilities:Card"], [("21/09/2026", "PAYMENT", "250.00")], "c.csv")
    current = _import(
        client, accounts["Assets:Current"], [("20/09/2026", "AMEX", "-250.00")], "d.csv"
    )
    assert current[0]["transfer_peer_id"] is None
    assert jev_off.requests == []


def test_a_failing_model_never_breaks_the_import(client, accounts, jev):
    jev.status = 503
    _import(client, accounts["Liabilities:Card"], [("21/09/2026", "PAYMENT", "250.00")], "c.csv")
    current = _import(
        client, accounts["Assets:Current"], [("20/09/2026", "AMEX", "-250.00")], "d.csv"
    )
    assert current[0]["transfer_peer_id"] is None
    logs = client.get("/api/ai/logs").json()["logs"]
    assert any(log["operation"] == "transfer_match" and log["error"] for log in logs)


# Card taps


@pytest.fixture(autouse=True)
def same_session(db_session, monkeypatch):
    """The background reconcile opens its own session; in tests it must share the test's."""
    monkeypatch.setattr(tap_service, "SessionLocal", lambda: nullcontext(db_session))


def _tap(client, merchant, amount="12.50", day="2026-09-20"):
    body = {"merchant": merchant, "amount": f"£{amount}", "card": "My Visa", "date": day}
    return client.post("/api/taps", json=body, headers=AUTH).json()


def _two_lines(client):
    """Two statement lines a "Bean There Coffee" tap could be, neither sharing a word with it,
    then the card paired (which reconciles)."""
    card_id, groceries_id, source_id = _setup(client)
    lines = [
        _statement_line(client, card_id, groceries_id, source_id, day, description, "12.50")
        for day, description in [("2026-09-20", "SQ *BTC LTD 0231"), ("2026-09-21", "PAYPAL *ZX")]
    ]
    client.put("/api/taps/cards", json={"card_name": "My Visa", "account_id": card_id})
    return lines


def test_the_model_picks_between_taps_candidates(client, jev):
    jev.judge = lambda tap, line: 0.92 if "BTC" in line["description"] else 0.05
    _tap(client, "Bean There Coffee")
    bean, _ = _two_lines(client)  # pairing the card reconciles
    [tap] = client.get("/api/taps", params={"status": "reconciled"}).json()
    assert tap["transaction_id"] == bean["id"]
    sent = jev.requests[0]["state"]
    assert sent["p0"]["a"]["merchant"] == "Bean There Coffee"
    assert {sent["p0"]["b"]["description"], sent["p1"]["b"]["description"]} == {
        "SQ *BTC LTD 0231",
        "PAYPAL *ZX",
    }


def test_a_late_tap_is_matched_after_the_reply(client, jev):
    jev.judge = lambda tap, line: 0.92 if "BTC" in line["description"] else 0.05
    bean, _ = _two_lines(client)
    # The reply doesn't wait for the model; the background reconcile links it
    assert _tap(client, "Bean There Coffee")["transaction_id"] is None
    [tap] = client.get("/api/taps", params={"status": "reconciled"}).json()
    assert tap["transaction_id"] == bean["id"]


def test_taps_stay_unmatched_when_the_model_is_unsure(client, jev):
    jev.judge = lambda tap, line: 0.3
    _tap(client, "Bean There Coffee")
    _two_lines(client)
    client.post("/api/taps/reconcile")
    assert client.get("/api/taps", params={"status": "reconciled"}).json() == []


def test_without_the_model_taps_fall_back_to_the_closest_date(client, jev_off):
    _tap(client, "Bean There Coffee")
    bean, _ = _two_lines(client)
    client.post("/api/taps/reconcile")
    [tap] = client.get("/api/taps", params={"status": "reconciled"}).json()
    assert tap["transaction_id"] == bean["id"]
    assert jev_off.requests == []


def test_on_demand_matching_waits_for_the_button(client, accounts, jev, db_session):
    use_model(db_session, "matching", "typesafe", "jev-latest", mode="on_demand")
    jev.judge = lambda a, b: 0.95 if "TESCO" in a["description"].upper() else 0.1
    current = accounts["Assets:Current"]
    first = _import(client, current, [("20/09/2026", "TESCO STORES 2231", "-12.50")], "a.csv")
    [second] = _import(client, current, [("22/09/2026", "Tesco Stores London", "-12.50")], "b.csv")
    assert second["duplicate_of"] is None
    assert jev.requests == []

    batch_id = max(b["id"] for b in client.get("/api/imports/batches").json())
    assert client.get(f"/api/imports/batches/{batch_id}").json()["batch"]["matched_at"] is None
    response = client.post(f"/api/imports/batches/{batch_id}/match")
    assert response.json() == {"duplicates": 1, "transfers": 0, "taps": 0}

    review = client.get(f"/api/imports/batches/{batch_id}").json()
    assert review["transactions"][0]["duplicate_of"]["transaction_id"] == first[0]["id"]
    assert review["batch"]["matched_at"] is not None


def test_matching_on_demand_needs_the_task_on(client, accounts, jev_off):
    _import(client, accounts["Assets:Current"], [("20/09/2026", "TFL", "-3.10")], "a.csv")
    batch_id = max(b["id"] for b in client.get("/api/imports/batches").json())
    response = client.post(f"/api/imports/batches/{batch_id}/match")
    assert response.status_code == 400
    assert jev_off.requests == []
