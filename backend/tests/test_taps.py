from datetime import date
from decimal import Decimal

import pytest

from pennychest.settings.models import AppSetting
from pennychest.taps.service import parse_amount, parse_tap_date
from pennychest.taps.token import TAP_TOKEN_KEY, clear_tap_token

TOKEN = "test-token"
AUTH = {"Authorization": f"Bearer {TOKEN}"}


@pytest.fixture(autouse=True)
def api_token(db_session):
    db_session.add(AppSetting(key=TAP_TOKEN_KEY, value=TOKEN))
    db_session.commit()


def _setup(client):
    card = client.post("/api/accounts", json={
        "name": "Visa", "full_path": "Liabilities:CreditCard:Visa", "type": "liability",
    }).json()
    groceries = client.post("/api/accounts", json={
        "name": "Groceries", "full_path": "Expenses:Groceries", "type": "expense",
    }).json()
    sources = client.get("/api/lookup/categorisation-sources").json()
    source_id = next(s["id"] for s in sources if s["name"] == "manual")
    return card["id"], groceries["id"], source_id


def _statement_line(client, card_id, expense_id, source_id, day, description, amount):
    """Create a transaction as a statement import would: card side negative for a purchase."""
    return client.post("/api/transactions", json={
        "date": day,
        "description": description,
        "postings": [
            {"account_id": card_id, "amount": f"-{amount}", "categorised_by_id": source_id},
            {"account_id": expense_id, "amount": amount, "categorised_by_id": source_id},
        ],
    }).json()


def _tap(client, **overrides):
    body = {"merchant": "Tesco", "amount": "£12.50", "card": "My Visa", "date": "2026-09-20"}
    body.update(overrides)
    return client.post("/api/taps", json=body, headers=AUTH)


@pytest.mark.parametrize("value,expected", [
    ("£12.50", (Decimal("12.50"), "GBP")),
    ("12,50 €", (Decimal("12.50"), "EUR")),
    ("$1,250.00", (Decimal("1250.00"), "USD")),
    ("1.250,75 EUR", (Decimal("1250.75"), "EUR")),
    ("-3.00", (Decimal("-3.00"), None)),
    (7.5, (Decimal("7.5"), None)),
])
def test_parse_amount(value, expected):
    assert parse_amount(value) == expected


@pytest.mark.parametrize("value,expected", [
    ("2026-09-26", date(2026, 9, 26)),
    ("2026-09-26T17:02:13+01:00", date(2026, 9, 26)),
    ("26 Sep 2026 at 17:02", date(2026, 9, 26)),
    ("26 September 2026", date(2026, 9, 26)),
    ("26/09/2026", date(2026, 9, 26)),
])
def test_parse_tap_date(value, expected):
    assert parse_tap_date(value) == expected


def test_ingest_requires_token(client):
    assert client.post("/api/taps", json={"merchant": "Tesco", "amount": "1"}).status_code == 401
    bad = {"Authorization": "Bearer wrong"}
    assert client.post("/api/taps", json={"merchant": "Tesco", "amount": "1"}, headers=bad).status_code == 401


def test_ingest_disabled_without_token(client, db_session):
    clear_tap_token(db_session)
    assert _tap(client).status_code == 503


def test_token_endpoints_require_session(anon_client):
    assert anon_client.get("/api/taps/token").status_code == 401
    assert anon_client.post("/api/taps/token").status_code == 401
    assert anon_client.delete("/api/taps/token").status_code == 401


def test_read_regenerate_and_clear_token(client):
    assert client.get("/api/taps/token").json() == {"token": TOKEN}

    new_token = client.post("/api/taps/token").json()["token"]
    assert new_token and new_token != TOKEN
    assert client.get("/api/taps/token").json() == {"token": new_token}
    assert _tap(client).status_code == 401
    headers = {"Authorization": f"Bearer {new_token}"}
    body = {"merchant": "Tesco", "amount": "£1.00"}
    assert client.post("/api/taps", json=body, headers=headers).status_code == 201

    assert client.delete("/api/taps/token").status_code == 204
    assert client.get("/api/taps/token").json() == {"token": None}
    assert client.post("/api/taps", json=body, headers=headers).status_code == 503


def test_ingest_validates_payload(client):
    assert _tap(client, merchant="").status_code == 400
    assert _tap(client, amount="abc").status_code == 400
    assert _tap(client, date="not a date").status_code == 400


def test_ingest_and_list(client):
    response = _tap(client)
    assert response.status_code == 201
    tap = response.json()
    assert tap["status"] == "unmatched"
    assert tap["currency"] == "GBP"
    assert Decimal(tap["amount"]) == Decimal("12.50")
    assert tap["account_id"] is None

    taps = client.get("/api/taps").json()
    assert [t["id"] for t in taps] == [tap["id"]]
    cards = client.get("/api/taps/cards").json()
    assert cards == [{"card_name": "My Visa", "account_id": None, "account_full_path": None, "tap_count": 1}]


def test_pairing_reconciles_existing_statement_lines(client):
    card_id, groceries_id, source_id = _setup(client)
    txn = _statement_line(client, card_id, groceries_id, source_id, "2026-09-22", "TESCO STORES 1234", "12.50")
    tap = _tap(client).json()
    assert tap["status"] == "unmatched"

    paired = client.put("/api/taps/cards", json={"card_name": "My Visa", "account_id": card_id})
    assert paired.status_code == 200
    assert paired.json()["account_full_path"] == "Liabilities:CreditCard:Visa"

    reconciled = client.get("/api/taps", params={"status": "reconciled"}).json()
    assert len(reconciled) == 1
    assert reconciled[0]["transaction_id"] == txn["id"]
    assert reconciled[0]["transaction_description"] == "TESCO STORES 1234"


def test_tap_after_statement_reconciles_on_ingest(client):
    card_id, groceries_id, source_id = _setup(client)
    client.put("/api/taps/cards", json={"card_name": "My Visa", "account_id": card_id})
    _statement_line(client, card_id, groceries_id, source_id, "2026-09-21", "TESCO", "12.50")
    assert _tap(client).json()["status"] == "reconciled"


def test_no_match_outside_window_or_wrong_amount(client):
    card_id, groceries_id, source_id = _setup(client)
    client.put("/api/taps/cards", json={"card_name": "My Visa", "account_id": card_id})
    _statement_line(client, card_id, groceries_id, source_id, "2026-10-05", "TESCO", "12.50")
    _statement_line(client, card_id, groceries_id, source_id, "2026-09-21", "TESCO", "12.51")
    assert _tap(client).json()["status"] == "unmatched"


def test_prefers_merchant_match_and_links_each_transaction_once(client):
    card_id, groceries_id, source_id = _setup(client)
    client.put("/api/taps/cards", json={"card_name": "My Visa", "account_id": card_id})
    other = _statement_line(client, card_id, groceries_id, source_id, "2026-09-20", "PRET A MANGER", "12.50")
    tesco = _statement_line(client, card_id, groceries_id, source_id, "2026-09-23", "TESCO STORES", "12.50")

    first = _tap(client).json()
    assert first["transaction_id"] == tesco["id"]
    # A second identical tap can only take the remaining transaction
    second = _tap(client, merchant="Pret").json()
    assert second["transaction_id"] == other["id"]


def test_foreign_currency_taps_are_not_auto_matched(client):
    card_id, groceries_id, source_id = _setup(client)
    client.put("/api/taps/cards", json={"card_name": "My Visa", "account_id": card_id})
    _statement_line(client, card_id, groceries_id, source_id, "2026-09-21", "TESCO", "12.50")
    assert _tap(client, amount="12,50 €").json()["status"] == "unmatched"


def test_dismiss_and_restore(client):
    tap = _tap(client).json()
    dismissed = client.post(f"/api/taps/{tap['id']}/dismiss").json()
    assert dismissed["status"] == "dismissed"
    assert client.get("/api/taps").json() == []
    assert len(client.get("/api/taps", params={"status": "dismissed"}).json()) == 1

    restored = client.post(f"/api/taps/{tap['id']}/restore").json()
    assert restored["status"] == "unmatched"


def test_deleting_statement_transaction_unlinks_tap(client, db_session):
    card_id, groceries_id, source_id = _setup(client)
    client.put("/api/taps/cards", json={"card_name": "My Visa", "account_id": card_id})
    txn = _statement_line(client, card_id, groceries_id, source_id, "2026-09-21", "TESCO", "12.50")
    tap = _tap(client).json()
    assert tap["status"] == "reconciled"

    assert client.delete(f"/api/transactions/{txn['id']}").status_code == 204
    db_session.expire_all()
    assert client.get("/api/taps").json()[0]["status"] == "unmatched"


def test_pairing_rejects_expense_accounts_and_can_unpair(client):
    card_id, groceries_id, _ = _setup(client)
    _tap(client)
    bad = client.put("/api/taps/cards", json={"card_name": "My Visa", "account_id": groceries_id})
    assert bad.status_code == 400

    client.put("/api/taps/cards", json={"card_name": "My Visa", "account_id": card_id})
    unpaired = client.put("/api/taps/cards", json={"card_name": "My Visa", "account_id": None}).json()
    assert unpaired["account_id"] is None
    assert client.get("/api/taps").json()[0]["account_id"] is None


def test_suggests_category_from_rules(client):
    _, groceries_id, _ = _setup(client)
    match_types = client.get("/api/lookup/match-types").json()
    substring = next(m["id"] for m in match_types if m["name"] == "substring")
    client.post("/api/rules", json={
        "pattern": "tesco", "match_type_id": substring, "target_account_id": groceries_id,
        "priority": 0, "source": "manual", "description": "",
    })
    assert _tap(client).json()["suggested_account_full_path"] == "Expenses:Groceries"
