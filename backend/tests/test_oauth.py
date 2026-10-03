import base64
import hashlib
import secrets
import time
from urllib.parse import parse_qs, urlsplit

import pytest

from pennychest.mcp.models import AccessToken, OAuthCode
from tests.test_actions import ledger  # noqa: F401  (fixture)
from tests.test_mcp import rpc

REDIRECT = "https://claude.ai/api/mcp/auth_callback"


def pkce():
    verifier = secrets.token_urlsafe(48)
    challenge = (
        base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest()).rstrip(b"=").decode()
    )
    return verifier, challenge


def register(client, **overrides):
    body = {"redirect_uris": [REDIRECT], "client_name": "Claude", **overrides}
    response = client.post("/oauth/register", json=body)
    assert response.status_code == 201, response.text
    return response.json()


def approve(client, registered, challenge, scopes=(), state="xyz"):
    response = client.post(
        "/api/oauth/approve",
        json={
            "client_id": registered["client_id"],
            "redirect_uri": REDIRECT,
            "state": state,
            "code_challenge": challenge,
            "code_challenge_method": "S256",
            "response_type": "code",
            "scopes": list(scopes),
        },
    )
    assert response.status_code == 200, response.text
    redirect = urlsplit(response.json()["redirect"])
    assert f"{redirect.scheme}://{redirect.netloc}{redirect.path}" == REDIRECT
    return {k: v[0] for k, v in parse_qs(redirect.query).items()}


def exchange(client, registered, code, verifier, **extra):
    return client.post(
        "/oauth/token",
        data={
            "grant_type": "authorization_code",
            "code": code,
            "redirect_uri": REDIRECT,
            "code_verifier": verifier,
            "client_id": registered["client_id"],
            **extra,
        },
    )


def connect(client, scopes=()):
    registered = register(client)
    verifier, challenge = pkce()
    code = approve(client, registered, challenge, scopes)["code"]
    tokens = exchange(client, registered, code, verifier)
    assert tokens.status_code == 200, tokens.text
    return registered, tokens.json()


# Discovery


def test_unauthenticated_mcp_points_to_discovery(anon_client):
    response = anon_client.post("/mcp", json={"jsonrpc": "2.0", "id": 1, "method": "ping"})
    assert response.status_code == 401
    assert (
        'resource_metadata="http://testserver/.well-known/oauth-protected-resource/mcp"'
        in response.headers["www-authenticate"]
    )


def test_discovery_documents(anon_client):
    resource = anon_client.get("/.well-known/oauth-protected-resource/mcp").json()
    assert resource["resource"] == "http://testserver/mcp"
    assert resource["authorization_servers"] == ["http://testserver"]
    assert anon_client.get("/.well-known/oauth-protected-resource").json() == resource

    server = anon_client.get("/.well-known/oauth-authorization-server").json()
    assert server["issuer"] == "http://testserver"
    assert server["authorization_endpoint"] == "http://testserver/oauth/authorize"
    assert server["code_challenge_methods_supported"] == ["S256"]
    assert server["registration_endpoint"] == "http://testserver/oauth/register"


def test_public_address_follows_the_proxy_scheme(anon_client):
    metadata = anon_client.get(
        "/.well-known/oauth-protected-resource/mcp", headers={"X-Forwarded-Proto": "https"}
    ).json()
    assert metadata["resource"] == "https://testserver/mcp"


# Registration


@pytest.mark.parametrize(
    "uris",
    [[], ["http://evil.example/cb"], ["https://claude.ai/cb#frag"], ["not a url"]],
)
def test_registration_rejects_bad_redirects(anon_client, uris):
    response = anon_client.post("/oauth/register", json={"redirect_uris": uris})
    assert response.status_code == 400
    assert response.json()["error"] == "invalid_redirect_uri"


def test_registration_allows_localhost_for_desktop_clients(anon_client):
    registered = register(anon_client, redirect_uris=["http://127.0.0.1:33418/callback"])
    assert registered["token_endpoint_auth_method"] == "none"
    assert "client_secret" not in registered


# The consent page


def test_consent_needs_a_signed_in_user(anon_client):
    registered = register(anon_client)
    info = anon_client.get(
        "/api/oauth/authorize-info",
        params={"client_id": registered["client_id"], "redirect_uri": REDIRECT},
    )
    assert info.status_code == 401
    assert anon_client.post("/api/oauth/approve", json={}).status_code == 401


def test_consent_page_info_checks_the_redirect(client):
    registered = register(client)
    info = client.get(
        "/api/oauth/authorize-info",
        params={"client_id": registered["client_id"], "redirect_uri": REDIRECT},
    ).json()
    assert info == {"client_name": "Claude", "redirect_host": "claude.ai"}
    wrong = client.get(
        "/api/oauth/authorize-info",
        params={"client_id": registered["client_id"], "redirect_uri": "https://evil.example/cb"},
    )
    assert wrong.status_code == 400
    unknown = client.get(
        "/api/oauth/authorize-info", params={"client_id": "pcc_nope", "redirect_uri": REDIRECT}
    )
    assert unknown.status_code == 400


def test_denying_sends_the_user_back_with_an_error(client):
    registered = register(client)
    response = client.post(
        "/api/oauth/deny",
        json={"client_id": registered["client_id"], "redirect_uri": REDIRECT, "state": "s1"},
    ).json()
    query = parse_qs(urlsplit(response["redirect"]).query)
    assert query["error"] == ["access_denied"] and query["state"] == ["s1"]
    assert query["iss"] == ["http://testserver"]


def test_pkce_is_required(client):
    registered = register(client)
    response = client.post(
        "/api/oauth/approve",
        json={"client_id": registered["client_id"], "redirect_uri": REDIRECT, "state": "s"},
    ).json()
    assert parse_qs(urlsplit(response["redirect"]).query)["error"] == ["invalid_request"]


# Tokens


def test_full_flow_gives_a_working_scoped_token(client, ledger):  # noqa: F811
    registered, tokens = connect(client, ["organise"])
    assert tokens["token_type"] == "Bearer" and tokens["expires_in"] == 3600
    assert tokens["scope"] == "read organise"

    tools = {
        t["name"]
        for t in rpc(client, tokens["access_token"], "tools/list").json()["result"]["tools"]
    }
    assert "create_category" in tools and "delete_transactions" not in tools

    [listed] = client.get("/api/mcp/tokens").json()["tokens"]
    assert listed["kind"] == "connector" and listed["name"] == "Claude"
    assert listed["scopes"] == ["organise"]


def test_codes_are_single_use_and_checked(client):
    registered = register(client)
    verifier, challenge = pkce()
    code = approve(client, registered, challenge)["code"]

    assert exchange(client, registered, code, "wrong-verifier").json()["error"] == "invalid_grant"
    assert (
        exchange(client, registered, code, verifier, redirect_uri="https://claude.ai/other").json()[
            "error"
        ]
        == "invalid_grant"
    )
    assert exchange(client, registered, code, verifier).status_code == 200
    assert exchange(client, registered, code, verifier).json()["error"] == "invalid_grant"

    other = register(client)
    code = approve(client, registered, challenge)["code"]
    assert exchange(client, other, code, verifier).json()["error"] == "invalid_grant"


def test_expired_codes_are_refused(client, db_session):
    registered = register(client)
    verifier, challenge = pkce()
    code = approve(client, registered, challenge)["code"]
    for grant in db_session.query(OAuthCode):
        grant.expires_at = int(time.time()) - 1
    db_session.commit()
    assert exchange(client, registered, code, verifier).json()["error"] == "invalid_grant"


def test_access_tokens_expire_and_refresh_rotates(client, db_session):
    registered, tokens = connect(client)
    for token in db_session.query(AccessToken):
        token.expires_at = int(time.time()) - 1
    db_session.commit()
    assert rpc(client, tokens["access_token"], "ping").status_code == 401

    refresh = {"grant_type": "refresh_token", "client_id": registered["client_id"]}
    renewed = client.post(
        "/oauth/token", data={**refresh, "refresh_token": tokens["refresh_token"]}
    )
    assert renewed.status_code == 200
    renewed = renewed.json()
    assert rpc(client, renewed["access_token"], "ping").status_code == 200
    # The old refresh token was replaced
    reused = client.post("/oauth/token", data={**refresh, "refresh_token": tokens["refresh_token"]})
    assert reused.json()["error"] == "invalid_grant"
    assert len(client.get("/api/mcp/tokens").json()["tokens"]) == 1


def test_revoking_in_settings_disconnects_the_app(client):
    registered, tokens = connect(client)
    [listed] = client.get("/api/mcp/tokens").json()["tokens"]
    client.delete(f"/api/mcp/tokens/{listed['id']}")
    assert rpc(client, tokens["access_token"], "ping").status_code == 401
    refreshed = client.post(
        "/oauth/token",
        data={
            "grant_type": "refresh_token",
            "client_id": registered["client_id"],
            "refresh_token": tokens["refresh_token"],
        },
    )
    assert refreshed.json()["error"] == "invalid_grant"


def test_clients_can_revoke_their_own_tokens(client):
    registered, tokens = connect(client)
    response = client.post(
        "/oauth/revoke",
        data={"token": tokens["refresh_token"], "client_id": registered["client_id"]},
    )
    assert response.status_code == 200
    assert rpc(client, tokens["access_token"], "ping").status_code == 401


def test_confidential_clients_must_send_their_secret(client):
    registered = register(client, token_endpoint_auth_method="client_secret_basic")
    secret = registered["client_secret"]
    verifier, challenge = pkce()
    code = approve(client, registered, challenge)["code"]
    data = {
        "grant_type": "authorization_code",
        "code": code,
        "redirect_uri": REDIRECT,
        "code_verifier": verifier,
    }
    no_secret = client.post("/oauth/token", data={**data, "client_id": registered["client_id"]})
    assert no_secret.status_code == 401 and no_secret.json()["error"] == "invalid_client"
    basic = base64.b64encode(f"{registered['client_id']}:{secret}".encode()).decode()
    ok = client.post("/oauth/token", data=data, headers={"Authorization": f"Basic {basic}"})
    assert ok.status_code == 200, ok.text


def test_unknown_grant_types_and_clients(client):
    assert client.post("/oauth/token", data={"grant_type": "password"}).status_code == 401
    registered = register(client)
    response = client.post(
        "/oauth/token", data={"grant_type": "password", "client_id": registered["client_id"]}
    )
    assert response.json()["error"] == "unsupported_grant_type"
    assert response.headers["cache-control"] == "no-store"


def test_pages_cannot_be_framed(anon_client):
    response = anon_client.get("/api/health")
    assert response.headers["x-frame-options"] == "DENY"
    assert response.headers["content-security-policy"] == "frame-ancestors 'none'"
