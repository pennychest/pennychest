import time

from fastapi.testclient import TestClient
from sqlalchemy import update

from pennychest.auth.models import AuthSession
from pennychest.auth.routes import COOKIE_NAME
from pennychest.auth.service import hash_password, login_throttle, verify_password
from pennychest.main import app
from pennychest.settings.models import AppSetting
from pennychest.taps.token import TAP_TOKEN_KEY
from tests.conftest import TEST_PASSWORD


def test_password_hash_round_trip():
    stored = hash_password("s3cret-password")
    assert stored != "s3cret-password"
    assert verify_password("s3cret-password", stored)
    assert not verify_password("wrong-password", stored)
    assert not verify_password("anything", "not-a-valid-hash")


def test_fresh_install_requires_setup(anon_client):
    assert anon_client.get("/api/auth/status").json() == {
        "setup_required": True,
        "authenticated": False,
    }


def test_api_requires_session(anon_client):
    assert anon_client.get("/api/accounts").status_code == 401
    assert anon_client.post("/api/accounts", json={}).status_code == 401
    assert anon_client.get("/api/ai/config").status_code == 401
    assert anon_client.get("/api/setup/status").status_code == 401


def test_health_is_public(anon_client):
    assert anon_client.get("/api/health").status_code == 200


def test_setup_rejects_short_password(anon_client):
    response = anon_client.post("/api/auth/setup", json={"password": "short"})
    assert response.status_code == 400
    assert anon_client.get("/api/auth/status").json()["setup_required"] is True


def test_setup_signs_in_and_can_only_run_once(client):
    assert client.get("/api/auth/status").json() == {
        "setup_required": False,
        "authenticated": True,
    }
    assert client.get("/api/accounts").status_code == 200

    response = client.post("/api/auth/setup", json={"password": "a-different-password"})
    assert response.status_code == 409


def test_session_cookie_flags(anon_client):
    response = anon_client.post("/api/auth/setup", json={"password": TEST_PASSWORD})
    cookie = response.headers["set-cookie"]
    assert "HttpOnly" in cookie
    assert "SameSite=lax" in cookie
    assert "Secure" not in cookie

    anon_client.post("/api/auth/logout")
    response = anon_client.post(
        "/api/auth/login",
        json={"password": TEST_PASSWORD},
        headers={"X-Forwarded-Proto": "https"},
    )
    assert "Secure" in response.headers["set-cookie"]


def test_login_and_logout(client):
    client.post("/api/auth/logout")
    assert client.get("/api/accounts").status_code == 401

    assert client.post("/api/auth/login", json={"password": "wrong-password"}).status_code == 401
    assert client.get("/api/accounts").status_code == 401

    assert client.post("/api/auth/login", json={"password": TEST_PASSWORD}).status_code == 204
    assert client.get("/api/accounts").status_code == 200


def test_login_before_setup(anon_client):
    assert anon_client.post("/api/auth/login", json={"password": TEST_PASSWORD}).status_code == 409


def test_logout_invalidates_the_token(client):
    token = client.cookies[COOKIE_NAME]
    client.post("/api/auth/logout")
    client.cookies.set(COOKIE_NAME, token)
    assert client.get("/api/accounts").status_code == 401


def test_forged_cookie_is_rejected(anon_client):
    anon_client.post("/api/auth/setup", json={"password": TEST_PASSWORD})
    anon_client.cookies.set(COOKIE_NAME, "not-a-real-token")
    assert anon_client.get("/api/accounts").status_code == 401


def test_expired_session_is_rejected(client, db_session):
    db_session.execute(update(AuthSession).values(expires_at=int(time.time()) - 1))
    assert client.get("/api/accounts").status_code == 401


def test_repeated_failures_are_throttled(client):
    client.post("/api/auth/logout")
    for _ in range(login_throttle.max_failures):
        response = client.post("/api/auth/login", json={"password": "wrong-password"})
        assert response.status_code == 401

    # Even the right password is refused until the window passes
    response = client.post("/api/auth/login", json={"password": TEST_PASSWORD})
    assert response.status_code == 429


def test_change_password_signs_out_other_devices(client):
    # Shares the database override that the `client` fixture installed
    other_device = TestClient(app)
    assert other_device.post("/api/auth/login", json={"password": TEST_PASSWORD}).status_code == 204
    assert other_device.get("/api/accounts").status_code == 200

    response = client.post(
        "/api/auth/password",
        json={"current_password": "wrong-password", "new_password": "new-password-123"},
    )
    assert response.status_code == 400

    response = client.post(
        "/api/auth/password",
        json={"current_password": TEST_PASSWORD, "new_password": "short"},
    )
    assert response.status_code == 400

    response = client.post(
        "/api/auth/password",
        json={"current_password": TEST_PASSWORD, "new_password": "new-password-123"},
    )
    assert response.status_code == 204

    assert client.get("/api/accounts").status_code == 200
    assert other_device.get("/api/accounts").status_code == 401

    client.post("/api/auth/logout")
    assert client.post("/api/auth/login", json={"password": TEST_PASSWORD}).status_code == 401
    assert client.post("/api/auth/login", json={"password": "new-password-123"}).status_code == 204


def test_tap_ingest_uses_api_token_not_session(anon_client, db_session):
    db_session.add(AppSetting(key=TAP_TOKEN_KEY, value="tap-token"))
    db_session.commit()
    body = {"merchant": "Tesco", "amount": "£3.00", "date": "2026-09-20"}

    assert anon_client.post("/api/taps", json=body).status_code == 401
    response = anon_client.post(
        "/api/taps", json=body, headers={"Authorization": "Bearer tap-token"}
    )
    assert response.status_code == 201

    # Reading taps is a normal signed-in action
    assert anon_client.get("/api/taps").status_code == 401
