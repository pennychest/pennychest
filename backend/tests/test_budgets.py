from datetime import date
from decimal import Decimal

import pytest

from pennychest.budgets.service import period_bounds


@pytest.fixture
def ledger(client):
    """A bank account plus Food (with a Groceries subcategory) and Transport."""
    ids = {}
    for name, path, type_ in [
        ("Bank", "Assets:Bank", "asset"),
        ("Expenses", "Expenses", "expense"),
        ("Food", "Expenses:Food", "expense"),
        ("Groceries", "Expenses:Food:Groceries", "expense"),
        ("Foodbank", "Expenses:Foodbank", "expense"),
        ("Transport", "Expenses:Transport", "expense"),
        ("Salary", "Income:Salary", "income"),
    ]:
        ids[path] = client.post(
            "/api/accounts",
            json={
                "name": name,
                "full_path": path,
                "type": type_,
            },
        ).json()["id"]
    sources = client.get("/api/lookup/categorisation-sources").json()
    ids["manual"] = next(s["id"] for s in sources if s["name"] == "manual")
    return ids


def _spend(client, ledger, category, amount, day, confirmed=True):
    txn = client.post(
        "/api/transactions",
        json={
            "date": day,
            "description": f"{category} {amount}",
            "postings": [
                {
                    "account_id": ledger["Assets:Bank"],
                    "amount": str(-Decimal(amount)),
                    "categorised_by_id": ledger["manual"],
                },
                {
                    "account_id": ledger[category],
                    "amount": amount,
                    "categorised_by_id": ledger["manual"],
                },
            ],
        },
    ).json()
    if confirmed:
        client.post(f"/api/transactions/{txn['id']}/confirm")
    return txn


def _budget(client, ledger, category, amount, period="monthly"):
    return client.post(
        "/api/budgets",
        json={
            "account_id": ledger[category],
            "amount": amount,
            "period": period,
        },
    )


def _by_category(budgets):
    return {b["account_full_path"]: b for b in budgets}


@pytest.mark.parametrize(
    "period,on,expected",
    [
        ("monthly", date(2026, 9, 15), (date(2026, 9, 1), date(2026, 10, 1))),
        ("monthly", date(2026, 12, 31), (date(2026, 12, 1), date(2027, 1, 1))),
        ("monthly", date(2028, 2, 29), (date(2028, 2, 1), date(2028, 3, 1))),
        ("annual", date(2026, 6, 1), (date(2026, 1, 1), date(2027, 1, 1))),
    ],
)
def test_period_bounds(period, on, expected):
    assert period_bounds(period, on) == expected


def test_budgets_require_session(anon_client):
    assert anon_client.get("/api/budgets").status_code == 401
    assert anon_client.post("/api/budgets", json={}).status_code == 401


def test_create_and_list_budget_with_spending(client, ledger):
    response = _budget(client, ledger, "Expenses:Food", "300")
    assert response.status_code == 201

    _spend(client, ledger, "Expenses:Food", "20.00", "2026-09-02")
    _spend(client, ledger, "Expenses:Food:Groceries", "45.50", "2026-09-10")
    _spend(client, ledger, "Expenses:Food:Groceries", "10.00", "2026-09-20", confirmed=False)
    _spend(client, ledger, "Expenses:Food", "99.00", "2026-08-31")  # previous month
    _spend(client, ledger, "Expenses:Transport", "15.00", "2026-09-05")  # other category
    _spend(client, ledger, "Expenses:Foodbank", "7.00", "2026-09-05")  # similar name only

    food = _by_category(client.get("/api/budgets", params={"on": "2026-09-15"}).json())[
        "Expenses:Food"
    ]
    assert food["period"] == "monthly"
    assert food["period_start"] == "2026-09-01" and food["period_end"] == "2026-09-30"
    assert Decimal(food["amount"]) == Decimal("300")
    assert Decimal(food["spent"]) == Decimal("75.50")
    assert Decimal(food["unreviewed"]) == Decimal("10.00")
    assert Decimal(food["remaining"]) == Decimal("224.50")

    august = _by_category(client.get("/api/budgets", params={"on": "2026-08-10"}).json())
    assert Decimal(august["Expenses:Food"]["spent"]) == Decimal("99.00")


def test_refunds_reduce_spending_and_overspend_goes_negative(client, ledger):
    _budget(client, ledger, "Expenses:Transport", "50")
    _spend(client, ledger, "Expenses:Transport", "80.00", "2026-09-03")
    _spend(client, ledger, "Expenses:Transport", "-10.00", "2026-09-04")

    transport = _by_category(client.get("/api/budgets", params={"on": "2026-09-30"}).json())
    assert Decimal(transport["Expenses:Transport"]["spent"]) == Decimal("70.00")
    assert Decimal(transport["Expenses:Transport"]["remaining"]) == Decimal("-20.00")


def test_annual_budget_covers_the_calendar_year(client, ledger):
    _budget(client, ledger, "Expenses:Transport", "1200", period="annual")
    _spend(client, ledger, "Expenses:Transport", "100.00", "2026-01-01")
    _spend(client, ledger, "Expenses:Transport", "200.00", "2026-12-31")
    _spend(client, ledger, "Expenses:Transport", "999.00", "2025-12-31")

    annual = client.get("/api/budgets", params={"on": "2026-06-01"}).json()[0]
    assert annual["period"] == "annual"
    assert annual["period_start"] == "2026-01-01" and annual["period_end"] == "2026-12-31"
    assert Decimal(annual["spent"]) == Decimal("300.00")


def test_budget_validation(client, ledger):
    assert _budget(client, ledger, "Assets:Bank", "100").status_code == 400
    assert _budget(client, ledger, "Income:Salary", "100").status_code == 400
    assert _budget(client, ledger, "Expenses:Food", "0").status_code == 422
    assert _budget(client, ledger, "Expenses:Food", "-5").status_code == 422
    assert client.post("/api/budgets", json={"account_id": 99999, "amount": "5"}).status_code == 404
    assert (
        client.post(
            "/api/budgets",
            json={
                "account_id": ledger["Expenses:Food"],
                "amount": "5",
                "period": "weekly",
            },
        ).status_code
        == 422
    )

    assert _budget(client, ledger, "Expenses:Food", "100").status_code == 201
    assert _budget(client, ledger, "Expenses:Food", "200").status_code == 409
    # The same category can have both a monthly and an annual budget.
    assert _budget(client, ledger, "Expenses:Food", "2000", period="annual").status_code == 201


def test_update_and_delete_budget(client, ledger):
    budget_id = _budget(client, ledger, "Expenses:Food", "100").json()["id"]
    _budget(client, ledger, "Expenses:Food", "900", period="annual")

    response = client.patch(f"/api/budgets/{budget_id}", json={"amount": "150"})
    assert response.status_code == 200 and Decimal(response.json()["amount"]) == Decimal("150")
    # Switching to annual would clash with the existing annual budget.
    assert client.patch(f"/api/budgets/{budget_id}", json={"period": "annual"}).status_code == 409
    assert client.patch("/api/budgets/99999", json={"amount": "1"}).status_code == 404

    assert client.delete(f"/api/budgets/{budget_id}").status_code == 204
    assert len(client.get("/api/budgets").json()) == 1
    assert client.delete(f"/api/budgets/{budget_id}").status_code == 404


def test_budget_follows_category_rename_and_deletion(client, ledger):
    _budget(client, ledger, "Expenses:Transport", "60")
    _spend(client, ledger, "Expenses:Transport", "12.00", date.today().isoformat())
    client.put(
        f"/api/accounts/{ledger['Expenses:Transport']}",
        json={
            "name": "Travel",
            "full_path": "Expenses:Travel",
        },
    )
    budget = client.get("/api/budgets").json()[0]
    assert budget["account_full_path"] == "Expenses:Travel"
    assert Decimal(budget["spent"]) == Decimal("12.00")


def test_suggestion_averages_recent_full_months(client, ledger):
    _spend(client, ledger, "Expenses:Food", "90.00", "2026-06-10")
    _spend(client, ledger, "Expenses:Food:Groceries", "60.00", "2026-07-10")
    _spend(client, ledger, "Expenses:Food", "30.00", "2026-08-10", confirmed=False)
    _spend(client, ledger, "Expenses:Food", "500.00", "2026-09-10")  # current month excluded
    _spend(client, ledger, "Expenses:Food", "500.00", "2026-05-31")  # too old

    response = client.get(
        "/api/budgets/suggestion",
        params={
            "account_id": ledger["Expenses:Food"],
            "on": "2026-09-20",
        },
    )
    assert response.status_code == 200
    body = response.json()
    assert [m["month"] for m in body["months"]] == ["2026-06", "2026-07", "2026-08"]
    assert Decimal(body["suggested"]) == Decimal("60.00")

    annual = client.get(
        "/api/budgets/suggestion",
        params={
            "account_id": ledger["Expenses:Food"],
            "period": "annual",
            "on": "2026-09-20",
        },
    ).json()
    assert len(annual["months"]) == 12
    assert Decimal(annual["suggested"]) == Decimal("680.00")

    assert (
        client.get(
            "/api/budgets/suggestion",
            params={
                "account_id": ledger["Income:Salary"],
            },
        ).status_code
        == 400
    )
