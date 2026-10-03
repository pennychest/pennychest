from tests.test_actions import ledger, ok, spending  # noqa: F401  (fixtures)


def test_cash_flow_by_month(client, spending):  # noqa: F811
    result = ok(client, "cash_flow", months=3, end="2026-09-15")
    assert result["months"] == [
        {"month": "2026-07", "income": "0.00", "expenses": "40.00", "net": "-40.00"},
        {"month": "2026-08", "income": "2000.00", "expenses": "135.00", "net": "1865.00"},
        {"month": "2026-09", "income": "0.00", "expenses": "0.00", "net": "0.00"},
    ]
    assert result["income_total"] == "2000.00" and result["expenses_total"] == "175.00"
    assert result["unreviewed_expenses"] == "10.00"
    # The same figures the chat gets from spending_summary
    summary = ok(
        client, "spending_summary", start_date="2026-08-01", end_date="2026-08-31"
    )
    assert summary["total"] == "135.00"


def test_net_worth_history(client, spending):  # noqa: F811
    result = ok(client, "net_worth_history", months=4, end="2026-09-15")
    assert result["currencies"]["GBP"] == [
        {"date": "2026-06-30", "net_worth": "0.00"},
        {"date": "2026-07-31", "net_worth": "-40.00"},
        {"date": "2026-08-31", "net_worth": "1825.00"},
        {"date": "2026-09-15", "net_worth": "1825.00"},
    ]


def test_layout_defaults_saves_and_resets(client):
    default = client.get("/api/dashboard/layout").json()["widgets"]
    assert [w["type"] for w in default][:4] == ["stat"] * 4

    layout = {
        "widgets": [
            {"id": "a", "type": "net_worth", "settings": {"months": 24}},
            {
                "id": "b",
                "type": "spending_trend",
                "settings": {"months": 12, "chart": "line", "categories": ["Expenses:Food"]},
            },
            {"id": "c", "type": "unmatched_taps"},
        ]
    }
    assert client.put("/api/dashboard/layout", json=layout).status_code == 200
    saved = client.get("/api/dashboard/layout").json()
    assert [w["id"] for w in saved["widgets"]] == ["a", "b", "c"]
    assert saved["widgets"][1]["settings"]["categories"] == ["Expenses:Food"]

    assert client.delete("/api/dashboard/layout").json() == {"widgets": default}
    assert client.get("/api/dashboard/layout").json() == {"widgets": default}


def test_layouts_are_checked(client):
    for bad in [
        {"widgets": [{"id": "a", "type": "pie_of_everything"}]},
        {"widgets": [{"id": "a", "type": "stat", "settings": {"stat": "mood"}}]},
        {"widgets": [{"id": "a", "type": "net_worth", "settings": {"colour": "red"}}]},
        {"widgets": [{"id": "a", "type": "net_worth", "settings": {"months": 1000}}]},
    ]:
        assert client.put("/api/dashboard/layout", json=bad).status_code == 422


def test_layout_needs_a_session(anon_client):
    assert anon_client.get("/api/dashboard/layout").status_code == 401


def test_transactions_can_leave_out_subcategories(client, spending):  # noqa: F811
    client.post(
        "/api/accounts", json={"name": "Food", "full_path": "Expenses:Food", "type": "expense"}
    )
    ok(
        client,
        "add_transaction",
        date="2026-08-15",
        description="FARM SHOP",
        amount="9.00",
        category="Expenses:Food",
        account="Assets:Bank:Current",
    )
    everything = ok(client, "list_transactions", category="Expenses:Food")
    own = ok(client, "list_transactions", category="Expenses:Food", exact_category=True)
    assert everything["total"] > own["total"] == 1
    assert own["transactions"][0]["description"] == "FARM SHOP"
