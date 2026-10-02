def _create_accounts(client):
    """Helper to create two accounts for transaction tests."""
    a1 = client.post("/api/accounts", json={
        "name": "Checking", "full_path": "Assets:Checking", "type": "asset",
    }).json()
    a2 = client.post("/api/accounts", json={
        "name": "Groceries", "full_path": "Expenses:Groceries", "type": "expense",
    }).json()
    return a1["id"], a2["id"]


def _get_manual_source_id(client):
    sources = client.get("/api/lookup/categorisation-sources").json()
    return next(s["id"] for s in sources if s["name"] == "manual")


def test_create_transaction(client):
    a1_id, a2_id = _create_accounts(client)
    source_id = _get_manual_source_id(client)

    response = client.post("/api/transactions", json={
        "date": "2026-01-15",
        "description": "Tesco groceries",
        "postings": [
            {"account_id": a2_id, "amount": "50.00", "categorised_by_id": source_id},
            {"account_id": a1_id, "amount": "-50.00", "categorised_by_id": source_id},
        ],
    })
    assert response.status_code == 201
    data = response.json()
    assert data["description"] == "Tesco groceries"
    assert len(data["postings"]) == 2


def test_transaction_must_balance(client):
    a1_id, a2_id = _create_accounts(client)
    source_id = _get_manual_source_id(client)

    response = client.post("/api/transactions", json={
        "date": "2026-01-15",
        "description": "Unbalanced",
        "postings": [
            {"account_id": a2_id, "amount": "50.00", "categorised_by_id": source_id},
            {"account_id": a1_id, "amount": "-30.00", "categorised_by_id": source_id},
        ],
    })
    assert response.status_code == 400


def test_transaction_needs_two_postings(client):
    a1_id, _ = _create_accounts(client)
    source_id = _get_manual_source_id(client)

    response = client.post("/api/transactions", json={
        "date": "2026-01-15",
        "description": "Single posting",
        "postings": [
            {"account_id": a1_id, "amount": "50.00", "categorised_by_id": source_id},
        ],
    })
    assert response.status_code == 400


def test_confirm_transaction(client):
    a1_id, a2_id = _create_accounts(client)
    source_id = _get_manual_source_id(client)

    txn = client.post("/api/transactions", json={
        "date": "2026-01-15",
        "description": "Pending transaction",
        "postings": [
            {"account_id": a2_id, "amount": "25.00", "categorised_by_id": source_id},
            {"account_id": a1_id, "amount": "-25.00", "categorised_by_id": source_id},
        ],
    }).json()
    assert txn["status"] == "pending"

    response = client.post(f"/api/transactions/{txn['id']}/confirm")
    assert response.status_code == 200
    assert response.json()["status"] == "confirmed"


def test_list_transactions_with_filters(client):
    a1_id, a2_id = _create_accounts(client)
    source_id = _get_manual_source_id(client)

    client.post("/api/transactions", json={
        "date": "2026-01-15",
        "description": "Searchable transaction",
        "postings": [
            {"account_id": a2_id, "amount": "10.00", "categorised_by_id": source_id},
            {"account_id": a1_id, "amount": "-10.00", "categorised_by_id": source_id},
        ],
    })

    # Search
    response = client.get("/api/transactions?search=Searchable")
    assert response.status_code == 200
    assert len(response.json()) >= 1

    # Status filter
    response = client.get("/api/transactions?status=pending")
    assert response.status_code == 200


def test_filter_by_category_includes_subcategories_and_dates(client):
    bank_id, _ = _create_accounts(client)
    source_id = _get_manual_source_id(client)
    food = client.post("/api/accounts", json={
        "name": "Food", "full_path": "Expenses:Food", "type": "expense",
    }).json()
    takeaway = client.post("/api/accounts", json={
        "name": "Takeaway", "full_path": "Expenses:Food:Takeaway", "type": "expense",
        "parent_id": food["id"],
    }).json()
    # A sibling whose name shares the prefix must not count as a subcategory
    foodbank = client.post("/api/accounts", json={
        "name": "Foodbank", "full_path": "Expenses:Foodbank", "type": "expense",
    }).json()

    def spend(day, description, category_id):
        client.post("/api/transactions", json={
            "date": day,
            "description": description,
            "postings": [
                {"account_id": category_id, "amount": "5.00", "categorised_by_id": source_id},
                {"account_id": bank_id, "amount": "-5.00", "categorised_by_id": source_id},
            ],
        })

    spend("2026-01-10", "Market", food["id"])
    spend("2026-02-10", "Pizza", takeaway["id"])
    spend("2026-02-11", "Donation", foodbank["id"])

    def descriptions(qs):
        response = client.get(f"/api/transactions?{qs}")
        assert response.status_code == 200
        return sorted(t["description"] for t in response.json())

    assert descriptions(f"category_id={food['id']}") == ["Market", "Pizza"]
    assert descriptions(f"category_id={takeaway['id']}") == ["Pizza"]
    assert descriptions(f"category_id={food['id']}&date_from=2026-02-01&date_to=2026-02-28") == ["Pizza"]
    assert client.get("/api/transactions?category_id=999999").status_code == 404


def test_count_matches_the_same_filters_as_the_list(client):
    bank_id, groceries_id = _create_accounts(client)
    source_id = _get_manual_source_id(client)
    for day, description in [("2026-01-10", "Tesco"), ("2026-02-10", "Tesco"), ("2026-02-11", "Petrol")]:
        client.post("/api/transactions", json={
            "date": day,
            "description": description,
            "postings": [
                {"account_id": groceries_id, "amount": "5.00", "categorised_by_id": source_id},
                {"account_id": bank_id, "amount": "-5.00", "categorised_by_id": source_id},
            ],
        })

    def count(qs):
        return client.get(f"/api/transactions/count?{qs}").json()["count"]

    assert count("") == 3
    assert count("search=tesco") == 2
    assert count("search=tesco&date_from=2026-02-01") == 1
    assert count(f"category_id={groceries_id}") == 3


def test_categorise_changes_the_category_side_of_a_hand_made_transaction(client):
    accounts = {}
    for path, kind in (
        ("Assets:Current", "asset"),
        ("Expenses:Food", "expense"),
        ("Expenses:Transport", "expense"),
    ):
        accounts[path] = client.post(
            "/api/accounts", json={"name": path.split(":")[-1], "full_path": path, "type": kind}
        ).json()["id"]
    sources = client.get("/api/lookup/categorisation-sources").json()
    manual = next(s["id"] for s in sources if s["name"] == "manual")
    # Category posting first, so "the posting that isn't first" would pick the bank side
    txn = client.post(
        "/api/transactions",
        json={
            "date": "2026-07-21",
            "description": "Bus",
            "postings": [
                {"account_id": accounts["Expenses:Food"], "amount": "3.30", "categorised_by_id": manual},
                {"account_id": accounts["Assets:Current"], "amount": "-3.30", "categorised_by_id": manual},
            ],
        },
    ).json()
    response = client.put(
        f"/api/imports/transactions/{txn['id']}/categorise",
        params={"target_account_id": accounts["Expenses:Transport"]},
    )
    assert response.status_code == 200
    paths = {p["account_full_path"] for p in client.get(f"/api/transactions/{txn['id']}").json()["postings"]}
    assert paths == {"Expenses:Transport", "Assets:Current"}
