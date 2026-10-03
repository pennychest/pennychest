import pytest

from tests.test_actions import ledger  # noqa: F401  (fixture)


def new_token(client, scopes=(), name="Claude Code"):
    response = client.post("/api/mcp/tokens", json={"name": name, "scopes": list(scopes)})
    assert response.status_code == 201, response.text
    return response.json()


def rpc(client, token, method, params=None, id_=1):
    body = {"jsonrpc": "2.0", "id": id_, "method": method}
    if params is not None:
        body["params"] = params
    return client.post(
        "/mcp",
        json=body,
        headers={
            "Authorization": f"Bearer {token}",
            "Accept": "application/json, text/event-stream",
        },
    )


def call(client, token, tool, /, **arguments):
    response = rpc(client, token, "tools/call", {"name": tool, "arguments": arguments})
    assert response.status_code == 200, response.text
    return response.json()["result"]


# Tokens


def test_tokens_are_shown_once_and_stored_hashed(client, db_session):
    from pennychest.mcp.models import AccessToken

    created = new_token(client, ["organise"])
    assert created["token"].startswith("pc_")
    assert created["prefix"] == created["token"][:10]
    assert created["scopes"] == ["organise"]

    [listed] = client.get("/api/mcp/tokens").json()["tokens"]
    assert "token" not in listed and listed["last_used_at"] is None
    stored = db_session.get(AccessToken, created["id"])
    assert created["token"] not in stored.token_hash


def test_token_validation(client):
    assert client.post("/api/mcp/tokens", json={"name": " ", "scopes": []}).status_code == 400
    bad = client.post("/api/mcp/tokens", json={"name": "x", "scopes": ["admin"]})
    assert bad.status_code == 400


def test_token_management_needs_a_session(anon_client):
    assert anon_client.get("/api/mcp/tokens").status_code == 401
    assert anon_client.post("/api/mcp/tokens", json={"name": "x"}).status_code == 401


def test_revoked_tokens_stop_working(client, anon_client):
    created = new_token(client)
    assert rpc(anon_client, created["token"], "ping").status_code == 200
    assert client.delete(f"/api/mcp/tokens/{created['id']}").status_code == 204
    assert rpc(anon_client, created["token"], "ping").status_code == 401


# The MCP protocol


@pytest.mark.parametrize("authorization", [None, "Bearer nope", "Bearer pc_wrong", "Basic abc"])
def test_requests_need_a_valid_token(anon_client, authorization):
    headers = {"Authorization": authorization} if authorization else {}
    response = anon_client.post(
        "/mcp", json={"jsonrpc": "2.0", "id": 1, "method": "ping"}, headers=headers
    )
    assert response.status_code == 401
    assert response.headers["www-authenticate"].startswith("Bearer")


def test_initialize_and_list_tools(anon_client, client):
    token = new_token(client)["token"]
    init = rpc(
        anon_client,
        token,
        "initialize",
        {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "t", "version": "1"},
        },
    ).json()["result"]
    assert init["protocolVersion"] == "2025-06-18"
    assert init["serverInfo"]["name"] == "PennyChest"
    assert "tools" in init["capabilities"]
    assert "read-only" in init["instructions"]

    unknown = rpc(anon_client, token, "initialize", {"protocolVersion": "1999-01-01"}).json()
    assert unknown["result"]["protocolVersion"] == "2025-11-25"

    notified = anon_client.post(
        "/mcp",
        json={"jsonrpc": "2.0", "method": "notifications/initialized"},
        headers={"Authorization": f"Bearer {token}"},
    )
    assert notified.status_code == 202

    tools = {t["name"]: t for t in rpc(anon_client, token, "tools/list").json()["result"]["tools"]}
    assert "list_transactions" in tools and "create_category" not in tools
    assert tools["list_transactions"]["annotations"]["readOnlyHint"] is True
    assert tools["list_transactions"]["inputSchema"]["type"] == "object"


def test_write_tokens_see_and_run_their_tools(anon_client, client, ledger):  # noqa: F811
    token = new_token(client, ["organise"])["token"]
    tools = {t["name"] for t in rpc(anon_client, token, "tools/list").json()["result"]["tools"]}
    assert "create_category" in tools and "delete_transactions" not in tools

    result = call(anon_client, token, "create_category", name="Gym", parent="Expenses")
    assert result["isError"] is False
    assert result["structuredContent"]["created"]["path"] == "Expenses:Gym"
    [change] = client.get("/api/changes").json()["changes"]
    assert change["source"] == "mcp"


def test_read_only_tokens_cannot_change_anything(anon_client, client, ledger):  # noqa: F811
    token = new_token(client)["token"]
    result = call(anon_client, token, "create_category", name="Gym", parent="Expenses")
    assert result["isError"] is True
    assert "permission" in result["content"][0]["text"]
    assert client.get("/api/changes").json()["changes"] == []


def test_tool_errors_and_protocol_errors(anon_client, client, ledger):  # noqa: F811
    token = new_token(client)["token"]
    bad_input = call(anon_client, token, "list_transactions", limit="lots")
    assert bad_input["isError"] is True and "Invalid input" in bad_input["content"][0]["text"]

    unknown_tool = rpc(anon_client, token, "tools/call", {"name": "launch_rockets"}).json()
    assert unknown_tool["error"]["code"] == -32602
    unknown_method = rpc(anon_client, token, "resources/list").json()
    assert unknown_method["error"]["code"] == -32601

    garbage = anon_client.post(
        "/mcp", content=b"{not json", headers={"Authorization": f"Bearer {token}"}
    )
    assert garbage.status_code == 400 and garbage.json()["error"]["code"] == -32700


def test_reading_data(anon_client, client, ledger):  # noqa: F811
    token = new_token(client)["token"]
    result = call(anon_client, token, "list_categories")
    assert any(c["path"] == "Expenses:Food" for c in result["structuredContent"]["categories"])
    assert client.get("/api/mcp/tokens").json()["tokens"][0]["last_used_at"] is not None


def test_no_stream_or_sessions(anon_client, client):
    token = new_token(client)["token"]
    headers = {"Authorization": f"Bearer {token}"}
    assert anon_client.get("/mcp", headers=headers).status_code == 405
    assert anon_client.delete("/mcp", headers=headers).status_code == 405


def test_a_signed_in_session_is_not_enough(client):
    response = client.post("/mcp", json={"jsonrpc": "2.0", "id": 1, "method": "ping"})
    assert response.status_code == 401


def test_initialize_lists_the_server_icon(anon_client, client):
    token = new_token(client)["token"]
    info = rpc(anon_client, token, "initialize", {"protocolVersion": "2025-11-25"}).json()["result"]
    icons = info["serverInfo"]["icons"]
    assert icons[0] == {
        "src": "http://testserver/pwa-512x512.png",
        "mimeType": "image/png",
        "sizes": ["512x512"],
    }
    assert info["serverInfo"]["websiteUrl"] == "http://testserver"
