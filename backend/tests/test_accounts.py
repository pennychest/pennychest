def test_create_account(client):
    response = client.post("/api/accounts", json={
        "name": "Bank",
        "full_path": "Assets:Bank",
        "type": "asset",
        "currency": "GBP",
    })
    assert response.status_code == 201
    data = response.json()
    assert data["name"] == "Bank"
    assert data["full_path"] == "Assets:Bank"
    assert data["type"] == "asset"


def test_list_accounts(client):
    client.post("/api/accounts", json={
        "name": "Assets",
        "full_path": "Assets",
        "type": "asset",
    })
    response = client.get("/api/accounts")
    assert response.status_code == 200
    assert len(response.json()) >= 1


def test_create_duplicate_account(client):
    client.post("/api/accounts", json={
        "name": "Assets",
        "full_path": "Assets",
        "type": "asset",
    })
    response = client.post("/api/accounts", json={
        "name": "Assets",
        "full_path": "Assets",
        "type": "asset",
    })
    assert response.status_code == 409


def test_delete_account(client):
    resp = client.post("/api/accounts", json={
        "name": "ToDelete",
        "full_path": "ToDelete",
        "type": "expense",
    })
    account_id = resp.json()["id"]
    delete_resp = client.delete(f"/api/accounts/{account_id}")
    assert delete_resp.status_code == 204
