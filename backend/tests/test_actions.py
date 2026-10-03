from datetime import date
from decimal import Decimal

import pytest

from pennychest.actions.catalog import (
    ACTIONS,
    ALL_SCOPES,
    Scope,
    ScopeNotAllowedError,
    describe_actions,
    run_action,
)
from pennychest.actions.insights import merchant_key
from pennychest.taps.models import CardTap

READ_ACTIONS = {
    "list_categories",
    "list_accounts",
    "spending_summary",
    "list_transactions",
    "list_rules",
    "get_budgets",
    "list_card_taps",
    "recurring_payments",
    "compare_periods",
    "savings_opportunities",
    "transactions_digest",
    "unusual_charges",
    "cash_flow",
    "net_worth_history",
}
ORGANISE_ACTIONS = {
    "create_category",
    "rename_category",
    "create_rule",
    "set_budget",
    "delete_budget",
}
TRANSACTION_ACTIONS = {
    "recategorise_transactions",
    "add_transaction",
    "add_transfer",
    "edit_transaction",
    "delete_transactions",
    "mark_reviewed",
}
WRITE_ACTIONS = ORGANISE_ACTIONS | TRANSACTION_ACTIONS


@pytest.fixture
def ledger(client):
    ids = {}
    for name, path, type_ in [
        ("Current", "Assets:Bank:Current", "asset"),
        ("Savings", "Assets:Bank:Savings", "asset"),
        ("Visa", "Liabilities:Visa", "liability"),
        ("Expenses", "Expenses", "expense"),
        ("Food", "Expenses:Food", "expense"),
        ("Groceries", "Expenses:Food:Groceries", "expense"),
        ("Restaurants", "Expenses:Food:Restaurants", "expense"),
        ("Transport", "Expenses:Transport", "expense"),
        ("Other", "Expenses:Other", "expense"),
        ("Uncategorised", "Expenses:Uncategorised", "expense"),
        ("Income", "Income", "income"),
        ("Salary", "Income:Salary", "income"),
        ("Other", "Income:Other", "income"),
    ]:
        ids[path] = client.post(
            "/api/accounts", json={"name": name, "full_path": path, "type": type_}
        ).json()["id"]
    sources = client.get("/api/lookup/categorisation-sources").json()
    ids["manual"] = next(s["id"] for s in sources if s["name"] == "manual")
    return ids


def _txn(client, ledger, day, description, postings, confirmed=True):
    txn = client.post(
        "/api/transactions",
        json={
            "date": day,
            "description": description,
            "postings": [
                {
                    "account_id": ledger[path],
                    "amount": amount,
                    "categorised_by_id": ledger["manual"],
                }
                for path, amount in postings
            ],
        },
    ).json()
    if confirmed:
        client.post(f"/api/transactions/{txn['id']}/confirm")
    return txn["id"]


def _spend(
    client,
    ledger,
    day,
    description,
    category,
    amount,
    paid_with="Assets:Bank:Current",
    confirmed=True,
):
    return _txn(
        client,
        ledger,
        day,
        description,
        [(paid_with, str(-Decimal(amount))), (category, amount)],
        confirmed,
    )


def act(client, action_name, /, **arguments):
    return client.post(f"/api/actions/{action_name}", json=arguments)


def ok(client, action_name, /, **arguments):
    response = act(client, action_name, **arguments)
    assert response.status_code == 200, response.text
    return response.json()


@pytest.fixture
def spending(client, ledger):
    """A few months of activity."""
    ids = {
        "tesco_jul": _spend(
            client, ledger, "2026-07-03", "TESCO STORES 1", "Expenses:Food:Groceries", "40.00"
        ),
        "tesco_aug": _spend(
            client, ledger, "2026-08-03", "TESCO STORES 1", "Expenses:Food:Groceries", "55.50"
        ),
        "pizza": _spend(
            client,
            ledger,
            "2026-08-10",
            "PIZZA 100% REAL",
            "Expenses:Food:Restaurants",
            "30.00",
            paid_with="Liabilities:Visa",
        ),
        "train": _spend(client, ledger, "2026-08-12", "TRAINLINE", "Expenses:Transport", "25.00"),
        "refund": _spend(
            client, ledger, "2026-08-20", "TESCO REFUND", "Expenses:Food:Groceries", "-5.50"
        ),
        "pending": _spend(
            client,
            ledger,
            "2026-08-25",
            "ALDI",
            "Expenses:Food:Groceries",
            "10.00",
            confirmed=False,
        ),
        "salary": _txn(
            client,
            ledger,
            "2026-08-28",
            "ACME PAYROLL",
            [("Assets:Bank:Current", "2000.00"), ("Income:Salary", "-2000.00")],
        ),
        "transfer": _txn(
            client,
            ledger,
            "2026-08-29",
            "TO SAVINGS",
            [("Assets:Bank:Current", "-100.00"), ("Assets:Bank:Savings", "100.00")],
        ),
        "split": _txn(
            client,
            ledger,
            "2026-08-30",
            "SUPERMARKET + TRAIN",
            [
                ("Assets:Bank:Current", "-20.00"),
                ("Expenses:Food:Groceries", "12.00"),
                ("Expenses:Transport", "8.00"),
            ],
        ),
    }
    return ids


# Catalogue


def test_catalogue_lists_every_action_with_strict_schemas(client):
    actions = {a["name"]: a for a in client.get("/api/actions").json()["actions"]}
    assert set(actions) == READ_ACTIONS | WRITE_ACTIONS == set(ACTIONS)
    for name, a in actions.items():
        expected = (
            "organise"
            if name in ORGANISE_ACTIONS
            else "transactions"
            if name in TRANSACTION_ACTIONS
            else "read"
        )
        assert a["scope"] == expected, name
        assert a["description"].strip(), name
        schema = a["input_schema"]
        assert schema["type"] == "object" and schema["additionalProperties"] is False, name


def test_actions_require_session(anon_client):
    assert anon_client.get("/api/actions").status_code == 401
    assert anon_client.post("/api/actions/list_categories", json={}).status_code == 401


def test_bad_calls_are_explained(client, ledger):
    assert act(client, "nope").status_code == 404
    response = act(client, "list_categories", colour="blue")
    assert response.status_code == 400 and "colour" in response.json()["detail"]
    response = act(client, "spending_summary", start_date="yesterday", end_date="2026-01-01")
    assert response.status_code == 400 and "start_date" in response.json()["detail"]
    response = act(client, "spending_summary", start_date="2026-02-01", end_date="2026-01-01")
    assert response.status_code == 400 and "end_date" in response.json()["detail"]


# Reading


def test_list_categories_and_accounts(client, ledger):
    paths = [c["path"] for c in ok(client, "list_categories")["categories"]]
    assert "Expenses:Food:Groceries" in paths and "Income:Salary" in paths
    assert "Assets:Bank:Current" not in paths
    income = ok(client, "list_categories", kind="income")["categories"]
    assert {c["type"] for c in income} == {"income"}
    accounts = [a["path"] for a in ok(client, "list_accounts")["accounts"]]
    assert accounts == ["Assets:Bank:Current", "Assets:Bank:Savings", "Liabilities:Visa"]


def test_spending_summary_by_category(client, spending):
    summary = ok(client, "spending_summary", start_date="2026-08-01", end_date="2026-08-31")
    groups = {g["key"]: g for g in summary["groups"]}
    assert summary["total"] == "135.00"  # 55.50 + 30 + 25 - 5.50 + 10 (pending) + 12 + 8
    assert summary["unreviewed"] == "10.00"
    assert groups["Expenses:Food:Groceries"]["total"] == "72.00"
    assert groups["Expenses:Food:Groceries"]["transactions"] == 4
    assert summary["groups"][0]["key"] == "Expenses:Food:Groceries"  # largest first

    rolled = ok(
        client, "spending_summary", start_date="2026-08-01", end_date="2026-08-31", category_depth=2
    )
    assert {g["key"]: g["total"] for g in rolled["groups"]} == {
        "Expenses:Food": "102.00",
        "Expenses:Transport": "33.00",
    }


def test_spending_summary_by_month_merchant_and_category_filter(client, spending):
    months = ok(
        client,
        "spending_summary",
        start_date="2026-07-01",
        end_date="2026-08-31",
        group_by="month",
        category="Food",
    )
    assert [(g["key"], g["total"]) for g in months["groups"]] == [
        ("2026-07", "40.00"),
        ("2026-08", "102.00"),
    ]
    merchants = ok(
        client,
        "spending_summary",
        start_date="2026-08-01",
        end_date="2026-08-31",
        group_by="merchant",
        limit=2,
    )
    assert [g["key"] for g in merchants["groups"]] == ["TESCO STORES 1", "PIZZA 100% REAL"]
    assert merchants["groups_not_shown"] == 4


def test_income_summary_is_positive(client, spending):
    income = ok(
        client, "spending_summary", start_date="2026-08-01", end_date="2026-08-31", kind="income"
    )
    assert income["total"] == "2000.00"
    assert income["groups"] == [{"key": "Income:Salary", "total": "2000.00", "transactions": 1}]


def test_list_transactions_filters(client, spending):
    everything = ok(client, "list_transactions")
    assert everything["total"] == 9
    assert everything["transactions"][0]["description"] == "SUPERMARKET + TRAIN"  # newest first

    pizza = ok(client, "list_transactions", search="100%")["transactions"]
    assert [t["description"] for t in pizza] == ["PIZZA 100% REAL"]
    assert pizza[0] == {
        "id": spending["pizza"],
        "date": "2026-08-10",
        "description": "PIZZA 100% REAL",
        "category": "Expenses:Food:Restaurants",
        "amount": "30.00",
        "account": "Liabilities:Visa",
        "status": "confirmed",
        "imported": False,
    }
    assert ok(client, "list_transactions", search="tesco")["total"] == 3

    food = ok(client, "list_transactions", category="Expenses:Food")
    assert food["total"] == 6
    visa = ok(client, "list_transactions", account="Visa")
    assert [t["id"] for t in visa["transactions"]] == [spending["pizza"]]
    pending = ok(client, "list_transactions", status="pending")
    assert [t["id"] for t in pending["transactions"]] == [spending["pending"]]
    big = ok(client, "list_transactions", min_amount="30", max_amount="60")
    assert {t["id"] for t in big["transactions"]} == {
        spending["tesco_jul"],
        spending["tesco_aug"],
        spending["pizza"],
    }
    august = ok(
        client,
        "list_transactions",
        start_date="2026-08-01",
        end_date="2026-08-15",
        limit=2,
        offset=1,
    )
    assert august["total"] == 3 and len(august["transactions"]) == 2

    transfer = next(t for t in everything["transactions"] if t["id"] == spending["transfer"])
    assert transfer["category"] is None and transfer["amount"] == "0.00"


def test_category_names_resolve_or_explain(client, spending):
    assert ok(client, "list_transactions", category="groceries")["total"] == 5
    response = act(client, "list_transactions", category="Other")
    assert response.status_code == 400
    assert (
        "Expenses:Other" in response.json()["detail"]
        and "Income:Other" in response.json()["detail"]
    )
    response = act(client, "list_transactions", category="Bank")
    assert response.status_code == 400 and "list_categories" in response.json()["detail"]


def test_card_taps(client, ledger, db_session):
    db_session.add_all(
        [
            CardTap(tapped_on=date(2026, 8, 1), merchant="Pret", amount=Decimal("4.20")),
            CardTap(
                tapped_on=date(2026, 8, 2), merchant="Boots", amount=Decimal("9.99"), dismissed=True
            ),
        ]
    )
    db_session.commit()
    unmatched = ok(client, "list_card_taps")["taps"]
    assert [(t["merchant"], t["amount"]) for t in unmatched] == [("Pret", "4.20")]
    assert len(ok(client, "list_card_taps", status="all")["taps"]) == 2
    assert [t["merchant"] for t in ok(client, "list_card_taps", status="dismissed")["taps"]] == [
        "Boots"
    ]


# Changing data


def test_create_category(client, ledger):
    created = ok(client, "create_category", name="Amazon Prime", parent="Food")["created"]
    assert created == {
        "path": "Expenses:Food:AmazonPrime",
        "name": "Amazon Prime",
        "type": "expense",
    }
    paths = [c["path"] for c in ok(client, "list_categories")["categories"]]
    assert "Expenses:Food:AmazonPrime" in paths

    assert act(client, "create_category", name="Amazon  Prime", parent="Food").status_code == 400
    assert act(client, "create_category", name="!!!", parent="Food").status_code == 400
    assert act(client, "create_category", name="Pocket", parent="Assets:Bank").status_code == 400
    income = ok(client, "create_category", name="Dividends", parent="Income")["created"]
    assert income["type"] == "income"


def test_rename_category_moves_subcategories(client, spending):
    ok(client, "set_budget", category="Food", amount="300")
    renamed = ok(client, "rename_category", category="Expenses:Food", new_name="Food & Drink")
    assert renamed["renamed"] == {
        "from": "Expenses:Food",
        "to": "Expenses:FoodDrink",
        "subcategories_moved": 2,
    }
    paths = [c["path"] for c in ok(client, "list_categories", kind="expense")["categories"]]
    assert "Expenses:FoodDrink:Groceries" in paths and "Expenses:Food:Groceries" not in paths
    assert ok(client, "list_transactions", category="Expenses:FoodDrink")["total"] == 6
    assert (
        ok(client, "get_budgets", on="2026-08-15")["budgets"][0]["category"] == "Expenses:FoodDrink"
    )

    assert (
        act(client, "rename_category", category="Uncategorised", new_name="Misc").status_code == 400
    )
    assert act(client, "rename_category", category="Expenses", new_name="Costs").status_code == 400
    assert (
        act(client, "rename_category", category="Transport", new_name="FoodDrink").status_code
        == 400
    )


def test_create_rule(client, ledger):
    created = ok(
        client,
        "create_rule",
        pattern="DELIVEROO",
        category="Restaurants",
        description="Food delivery",
    )["created"]
    assert created["category"] == "Expenses:Food:Restaurants" and created["priority"] == 10
    rules = ok(client, "list_rules")["rules"]
    assert rules == [
        {
            "id": created["id"],
            "pattern": "DELIVEROO",
            "match_type": "substring",
            "category": "Expenses:Food:Restaurants",
            "priority": 10,
            "description": "Food delivery",
        }
    ]
    assert client.get("/api/rules").json()[0]["source"] == "ai"

    assert (
        act(client, "create_rule", pattern="DELIVEROO", category="Restaurants").status_code == 400
    )
    response = act(client, "create_rule", pattern="(", category="Restaurants", match_type="regex")
    assert response.status_code == 400 and "regular expression" in response.json()["detail"]
    assert (
        act(client, "create_rule", pattern="X", category="Restaurants", priority=0).status_code
        == 400
    )


def test_recategorise_transactions(client, spending):
    result = ok(
        client,
        "recategorise_transactions",
        category="Transport",
        transaction_ids=[
            spending["tesco_aug"],
            spending["transfer"],
            spending["split"],
            999999,
            spending["tesco_aug"],
        ],
    )
    assert result["updated"] == [spending["tesco_aug"]]
    assert {s["id"]: s["reason"] for s in result["skipped"]} == {
        spending["transfer"]: "has no category (e.g. a transfer)",
        spending["split"]: "split across several categories",
        999999: "not found",
    }
    moved = client.get(f"/api/transactions/{spending['tesco_aug']}").json()
    category_posting = next(
        p for p in moved["postings"] if p["account_full_path"].startswith("Expenses")
    )
    assert category_posting["account_full_path"] == "Expenses:Transport"
    assert (
        act(
            client, "recategorise_transactions", category="Transport", transaction_ids=[]
        ).status_code
        == 400
    )


def test_set_and_delete_budgets(client, spending):
    first = ok(client, "set_budget", category="Food", amount="250")
    assert first["budget"] == {"category": "Expenses:Food", "period": "monthly", "limit": "250.00"}
    assert first["previous_limit"] is None
    assert ok(client, "set_budget", category="Food", amount="300")["previous_limit"] == "250.00"
    ok(client, "set_budget", category="Transport", amount="1000", period="annual")

    budgets = {b["category"]: b for b in ok(client, "get_budgets", on="2026-08-15")["budgets"]}
    assert budgets["Expenses:Food"]["spent"] == "102.00"
    assert budgets["Expenses:Food"]["remaining"] == "198.00"
    assert budgets["Expenses:Food"]["unreviewed"] == "10.00"
    assert budgets["Expenses:Transport"]["period_start"] == "2026-01-01"
    assert len(client.get("/api/budgets").json()) == 2

    assert act(client, "set_budget", category="Salary", amount="5").status_code == 400
    assert act(client, "set_budget", category="Food", amount="0").status_code == 400
    assert ok(client, "delete_budget", category="Food")["deleted"]["limit"] == "300.00"
    assert act(client, "delete_budget", category="Food").status_code == 400


# Insights


@pytest.fixture
def subscriptions(client, ledger, spending):
    """Regular charges on top of the spending fixture."""
    for day in ["2026-05-15", "2026-06-15", "2026-07-15", "2026-08-15", "2026-09-15"]:
        _spend(client, ledger, day, f"NETFLIX.COM {day[5:7]}88", "Expenses:Other", "10.99")
    for day in ["2026-03-01", "2026-04-01", "2026-05-01"]:  # cancelled
        _spend(client, ledger, day, "PUREGYM", "Expenses:Other", "25.00")
    for day in ["2025-10-01", "2026-09-25"]:
        _spend(client, ledger, day, "ACME INSURANCE", "Expenses:Other", "240.00")


def test_merchant_key_drops_references():
    assert merchant_key("NETFLIX.COM 0988") == merchant_key("NETFLIX.COM 1288") == "netflix com"
    assert merchant_key("AMAZON MKTPLACE*AB12CD") == "amazon mktplace"
    assert merchant_key("12345") == "12345"


def test_recurring_payments(client, subscriptions):
    result = ok(client, "recurring_payments", on="2026-09-30")
    assert [(p["merchant"], p["cadence"], p["yearly_cost"]) for p in result["payments"]] == [
        ("ACME INSURANCE", "yearly", "240.00"),
        ("NETFLIX.COM 0988", "monthly", "131.88"),
    ]
    netflix = result["payments"][1]
    assert netflix["charges"] == 5 and netflix["category"] == "Expenses:Other"
    assert netflix["amount"] == "10.99" and not netflix["amount_varies"]
    assert result["yearly_total"] == "371.88"
    # Tesco only appears twice, and the gym stopped in May.


def test_compare_periods(client, subscriptions):
    result = ok(
        client,
        "compare_periods",
        start_date="2026-08-01",
        end_date="2026-08-31",
        category_depth=2,
    )
    assert result["compared_with"]["start_date"] == "2026-07-01"
    assert result["compared_with"]["end_date"] == "2026-07-31"
    assert result["change"] == "95.00"  # 145.99 - 50.99
    assert [(g["key"], g["total"], g["compared_total"], g["change"]) for g in result["groups"]] == [
        ("Expenses:Food", "102.00", "40.00", "62.00"),
        ("Expenses:Transport", "33.00", "0.00", "33.00"),
        ("Expenses:Other", "10.99", "10.99", "0.00"),
    ]
    explicit = ok(
        client,
        "compare_periods",
        start_date="2026-08-01",
        end_date="2026-08-31",
        compare_start_date="2026-05-01",
        compare_end_date="2026-05-31",
        group_by="merchant",
        limit=1,
    )
    assert explicit["compared_with"]["total"] == "35.99"  # Netflix and the gym
    assert explicit["groups"][0]["key"] == "TESCO STORES 1"
    bad = act(
        client,
        "compare_periods",
        start_date="2026-08-01",
        end_date="2026-08-31",
        compare_start_date="2026-07-01",
    )
    assert bad.status_code == 400


def test_savings_opportunities(client, subscriptions):
    ok(client, "set_budget", category="Expenses:Other", amount="5")
    result = ok(client, "savings_opportunities", months=2, on="2026-09-28")
    assert result["averaged_over"] == {
        "start_date": "2026-07-01",
        "end_date": "2026-08-31",
        "months": 2,
    }
    assert result["monthly_average_spending"] == "98.49"  # (50.99 + 145.99) / 2
    assert result["unreviewed"] == "10.00"
    assert result["top_categories"][:2] == [
        {"category": "Expenses:Food", "monthly_average": "71.00"},
        {"category": "Expenses:Transport", "monthly_average": "16.50"},
    ]
    assert result["top_merchants"][0] == {
        "merchant": "TESCO STORES 1",
        "monthly_average": "47.75",
        "transactions": 2,
    }
    # August against the June-July average; Other didn't change so isn't listed.
    assert result["rising_last_month"] == {
        "month": "2026-08",
        "categories": [
            {
                "category": "Expenses:Food",
                "last_month": "102.00",
                "usual_month": "20.00",
                "increase": "82.00",
            },
            {
                "category": "Expenses:Transport",
                "last_month": "33.00",
                "usual_month": "0.00",
                "increase": "33.00",
            },
        ],
    }
    assert [p["merchant"] for p in result["recurring_payments"]] == [
        "ACME INSURANCE",
        "NETFLIX.COM 0988",
    ]
    assert result["recurring_yearly_total"] == "371.88"
    assert result["overspent_budgets"] == [
        {
            "category": "Expenses:Other",
            "period": "monthly",
            "limit": "5.00",
            "spent": "250.99",
            "over_by": "245.99",
        }
    ]


def test_transactions_digest_lists_every_transaction(client, spending):
    result = ok(client, "transactions_digest", start_date="2026-08-01", end_date="2026-08-31")
    assert result["detail"] == "transactions"
    assert result["transactions"] == 7  # the transfer to savings is left out
    assert result["total_spending"] == "135.00"
    assert result["total_income"] == "2000.00"
    assert result["unreviewed"] == "10.00"
    lines = result["csv"].splitlines()
    assert lines[0] == "date,description,amount,category,account,status"
    assert lines[1] == (
        "2026-08-03,TESCO STORES 1,55.50,Expenses:Food:Groceries,Assets:Bank:Current,confirmed"
    )
    assert '"PIZZA 100% REAL"' not in result["csv"]  # only quoted when needed
    assert (
        "2026-08-30,SUPERMARKET + TRAIN,20.00,Expenses:Food:Groceries;Expenses:Transport,"
        "Assets:Bank:Current,confirmed"
    ) in lines
    assert "2026-08-28,ACME PAYROLL,-2000.00,Income:Salary" in result["csv"]
    assert "TO SAVINGS" not in result["csv"]


def test_transactions_digest_rolls_up_by_merchant(client, subscriptions, monkeypatch):
    from pennychest.actions import insights

    monkeypatch.setattr(insights, "DIGEST_MAX_TRANSACTIONS", 5)
    result = ok(client, "transactions_digest", start_date="2026-05-01", end_date="2026-09-30")
    assert result["detail"] == "merchants"
    lines = result["csv"].splitlines()
    assert lines[0] == ("merchant,category,transactions,total,typical_amount,first_date,last_date")
    assert lines[1] == "ACME INSURANCE,Expenses:Other,1,240.00,240.00,2026-09-25,2026-09-25"
    assert "NETFLIX.COM 0988,Expenses:Other,5,54.95,10.99,2026-05-15,2026-09-15" in lines
    assert lines[-1].startswith("ACME PAYROLL,Income:Salary,1,-2000.00")
    forced = ok(
        client,
        "transactions_digest",
        start_date="2026-05-01",
        end_date="2026-09-30",
        detail="transactions",
    )
    assert forced["detail"] == "transactions"


# Permissions


def test_callers_are_read_only_by_default(ledger, db_session):
    with pytest.raises(ScopeNotAllowedError, match="'organise' permission"):
        run_action(db_session, "create_category", {"name": "Gym", "parent": "Expenses"})
    paths = [c["path"] for c in run_action(db_session, "list_categories", {})["categories"]]
    assert "Expenses:Gym" not in paths


def test_each_changing_action_needs_its_own_scope(db_session):
    # Refused before the input is even looked at.
    for name in ORGANISE_ACTIONS:
        for scopes in ((), ("transactions",)):
            with pytest.raises(ScopeNotAllowedError, match="'organise'"):
                run_action(db_session, name, {}, scopes=scopes)
    for name in TRANSACTION_ACTIONS:
        for scopes in ((), ("organise",)):
            with pytest.raises(ScopeNotAllowedError, match="'transactions'"):
                run_action(db_session, name, {}, scopes=scopes)


def test_organise_scope_cannot_touch_transactions(ledger, db_session):
    organiser = {"scopes": [Scope.ORGANISE]}
    created = run_action(
        db_session, "create_category", {"name": "Gym", "parent": "Expenses"}, **organiser
    )
    assert created["created"]["path"] == "Expenses:Gym"
    with pytest.raises(ScopeNotAllowedError):
        run_action(
            db_session,
            "recategorise_transactions",
            {"transaction_ids": [1], "category": "Gym"},
            **organiser,
        )


def test_transactions_scope_cannot_organise(client, spending, db_session):
    moved = run_action(
        db_session,
        "recategorise_transactions",
        {"transaction_ids": [spending["train"]], "category": "Groceries"},
        scopes=["transactions"],
    )
    assert moved["updated"] == [spending["train"]]
    with pytest.raises(ScopeNotAllowedError):
        run_action(
            db_session, "set_budget", {"category": "Food", "amount": "10"}, scopes=["transactions"]
        )


def test_catalogue_is_filtered_by_scope():
    def names(*scopes):
        return {a["name"] for a in describe_actions(scopes)}

    assert names() == READ_ACTIONS
    assert names("organise") == READ_ACTIONS | ORGANISE_ACTIONS
    assert names("transactions") == READ_ACTIONS | TRANSACTION_ACTIONS
    assert names(*ALL_SCOPES) == READ_ACTIONS | WRITE_ACTIONS
    with pytest.raises(ValueError):
        names("admin")


def test_signed_in_session_has_every_scope(client, spending):
    assert act(client, "create_category", name="Gym", parent="Expenses").status_code == 200
    response = act(
        client, "recategorise_transactions", transaction_ids=[spending["train"]], category="Gym"
    )
    assert response.status_code == 200


# Transactions


def _postings(client, txn_id):
    txn = client.get(f"/api/transactions/{txn_id}").json()
    return {p["account_full_path"]: p["amount"] for p in txn["postings"]}


@pytest.fixture
def imported(client, ledger, db_session):
    """A transaction as a statement import leaves it."""
    from pennychest.core.lookup_models import ImportSourceType
    from pennychest.imports.models import ImportBatch, RawImportRow
    from pennychest.transactions.models import Transaction

    source = db_session.query(ImportSourceType).filter_by(name="file").one()
    batch = ImportBatch(importer_name="csv", source_type_id=source.id)
    db_session.add(batch)
    db_session.commit()
    txn_id = _spend(
        client,
        ledger,
        "2026-08-05",
        "TESCO 123",
        "Expenses:Food:Groceries",
        "20.00",
        confirmed=False,
    )
    db_session.get(Transaction, txn_id).import_batch_id = batch.id
    db_session.add(
        RawImportRow(
            batch_id=batch.id,
            raw_content="05/08/2026,TESCO 123,-20.00",
            line_number=1,
            transaction_id=txn_id,
        )
    )
    db_session.commit()
    return txn_id


def test_add_spending_from_a_card(client, ledger):
    added = ok(
        client,
        "add_transaction",
        description="Cash coffee",
        amount="4.50",
        category="Restaurants",
        account="Visa",
        date="2026-08-20",
    )["added"]
    assert added == {
        "id": added["id"],
        "date": "2026-08-20",
        "description": "Cash coffee",
        "category": "Expenses:Food:Restaurants",
        "amount": "4.50",
        "account": "Liabilities:Visa",
        "status": "confirmed",
        "imported": False,
    }
    assert {k: float(v) for k, v in _postings(client, added["id"]).items()} == {
        "Expenses:Food:Restaurants": 4.5,
        "Liabilities:Visa": -4.5,
    }
    summary = ok(client, "spending_summary", start_date="2026-08-20", end_date="2026-08-20")
    assert summary["total"] == "4.50" and summary["unreviewed"] == "0.00"


def test_add_income_refund_and_default_date(client, ledger):
    income = ok(
        client,
        "add_transaction",
        description="Birthday money",
        amount="50",
        category="Income:Other",
        account="Current",
    )["added"]
    assert income["amount"] == "-50.00" and income["date"] == date.today().isoformat()
    assert float(_postings(client, income["id"])["Assets:Bank:Current"]) == 50.0

    ambiguous = act(
        client,
        "add_transaction",
        description="Returned shoes",
        amount="-30",
        category="Other",
        account="Current",
    )
    assert ambiguous.status_code == 400
    assert "Expenses:Other" in ambiguous.json()["detail"]
    assert ok(client, "list_transactions", search="Returned shoes")["total"] == 0


def test_add_transaction_validation(client, ledger):
    base = {"description": "x", "amount": "5", "category": "Groceries", "account": "Current"}
    assert act(client, "add_transaction", **{**base, "amount": "0"}).status_code == 400
    assert act(client, "add_transaction", **{**base, "category": "Current"}).status_code == 400
    assert act(client, "add_transaction", **{**base, "account": "Groceries"}).status_code == 400
    refund = ok(client, "add_transaction", **{**base, "amount": "-5"})["added"]
    assert refund["amount"] == "-5.00"
    assert float(_postings(client, refund["id"])["Assets:Bank:Current"]) == 5.0


def test_add_transfer(client, ledger):
    added = ok(
        client,
        "add_transfer",
        amount="100",
        from_account="Current",
        to_account="Savings",
        date="2026-08-31",
    )["added"]
    assert added["category"] is None and added["description"] == "Transfer"
    assert {k: float(v) for k, v in _postings(client, added["id"]).items()} == {
        "Assets:Bank:Current": -100.0,
        "Assets:Bank:Savings": 100.0,
    }
    assert (
        act(
            client, "add_transfer", amount="5", from_account="Current", to_account="Current"
        ).status_code
        == 400
    )
    assert (
        act(
            client, "add_transfer", amount="-5", from_account="Current", to_account="Savings"
        ).status_code
        == 400
    )


def test_edit_a_transaction_the_user_added(client, ledger):
    txn = ok(
        client,
        "add_transaction",
        description="Coffee",
        amount="3",
        category="Restaurants",
        account="Current",
        date="2026-08-01",
    )["added"]
    edited = ok(
        client,
        "edit_transaction",
        transaction_id=txn["id"],
        description="Lunch",
        date="2026-08-02",
        amount="12.40",
        account="Visa",
    )["edited"]
    assert (edited["description"], edited["date"], edited["amount"], edited["account"]) == (
        "Lunch",
        "2026-08-02",
        "12.40",
        "Liabilities:Visa",
    )
    assert {k: float(v) for k, v in _postings(client, txn["id"]).items()} == {
        "Expenses:Food:Restaurants": 12.4,
        "Liabilities:Visa": -12.4,
    }

    # Moving from spending to income keeps the amount's meaning and flips the entry.
    moved = ok(client, "edit_transaction", transaction_id=txn["id"], category="Salary")["edited"]
    assert moved["category"] == "Income:Salary" and moved["amount"] == "-12.40"
    assert float(_postings(client, txn["id"])["Liabilities:Visa"]) == 12.4


def test_edit_transfers_and_splits(client, spending):
    ok(client, "edit_transaction", transaction_id=spending["transfer"], amount="75")
    assert {k: float(v) for k, v in _postings(client, spending["transfer"]).items()} == {
        "Assets:Bank:Current": -75.0,
        "Assets:Bank:Savings": 75.0,
    }
    assert (
        act(
            client, "edit_transaction", transaction_id=spending["transfer"], category="Groceries"
        ).status_code
        == 400
    )
    assert (
        act(client, "edit_transaction", transaction_id=spending["split"], amount="30").status_code
        == 400
    )
    renamed = ok(
        client, "edit_transaction", transaction_id=spending["split"], description="Weekly shop"
    )["edited"]
    assert renamed["description"] == "Weekly shop"
    assert act(client, "edit_transaction", transaction_id=999999, amount="1").status_code == 400


def _statement_row(db_session):
    from pennychest.imports.models import RawImportRow

    db_session.expire_all()
    return db_session.query(RawImportRow).filter_by(raw_content="05/08/2026,TESCO 123,-20.00").one()


def test_imported_transactions_can_be_edited(client, imported, db_session):
    edited = ok(
        client,
        "edit_transaction",
        transaction_id=imported,
        amount="25",
        date="2026-08-06",
        description="Tesco groceries",
        account="Visa",
        category="Transport",
    )["edited"]
    assert edited == {
        "id": imported,
        "date": "2026-08-06",
        "description": "Tesco groceries",
        "category": "Expenses:Transport",
        "amount": "25.00",
        "account": "Liabilities:Visa",
        "status": "pending",
        "imported": True,
    }
    # What the bank reported is still stored with the import.
    assert _statement_row(db_session).transaction_id == imported


def test_edit_descriptions_steer_models_away_from_bank_details():
    descriptions = {a["name"]: a["description"] for a in describe_actions(ALL_SCOPES)}
    assert (
        "only change their amount, date, account or description when the user asks"
        in (descriptions["edit_transaction"])
    )
    assert "only delete one when the user asks for it" in descriptions["delete_transactions"]


def test_delete_transactions(client, spending, imported):
    manual = ok(
        client,
        "add_transaction",
        description="Oops",
        amount="1",
        category="Groceries",
        account="Current",
    )["added"]["id"]
    result = ok(
        client,
        "delete_transactions",
        transaction_ids=[manual, spending["transfer"], imported, 999999],
    )
    assert [t["id"] for t in result["deleted"]] == [manual, spending["transfer"], imported]
    assert result["skipped"] == [{"id": 999999, "reason": "not found"}]
    for txn_id in (manual, spending["transfer"], imported):
        assert client.get(f"/api/transactions/{txn_id}").status_code == 404


def test_deleting_imported_transactions_keeps_the_statement_row(client, imported, db_session):
    ok(client, "delete_transactions", transaction_ids=[imported])
    row = _statement_row(db_session)
    assert row.transaction_id is None and row.line_number == 1


def test_app_can_delete_imported_transactions(client, imported, db_session):
    # This used to fail with a foreign key error from the statement row.
    assert client.delete(f"/api/transactions/{imported}").status_code == 204
    assert client.get(f"/api/transactions/{imported}").status_code == 404
    assert _statement_row(db_session).transaction_id is None


def test_mark_reviewed(client, spending, imported):
    result = ok(
        client, "mark_reviewed", transaction_ids=[imported, spending["pending"], spending["pizza"]]
    )
    assert result["reviewed"] == [imported, spending["pending"]]
    assert result["skipped"] == [{"id": spending["pizza"], "reason": "already reviewed"}]
    assert ok(client, "list_transactions", status="pending")["total"] == 0
