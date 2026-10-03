"""OAuth 2.1 for the MCP server, so web connectors (claude.ai, ChatGPT) can sign in.

Follows the MCP authorization spec: protected resource metadata (RFC 9728) points connectors at
this server's authorization server metadata (RFC 8414); they register themselves (RFC 7591),
send the user to PennyChest's consent page, and exchange the code for tokens with PKCE (S256).
Access tokens last an hour and are renewed with a refresh token that rotates on every use.
"""

import base64
import hashlib
import secrets
import time
from urllib.parse import urlencode, urlsplit

from fastapi import APIRouter, Depends, Form, Header, HTTPException, Request
from fastapi.responses import JSONResponse
from pydantic import BaseModel
from sqlalchemy import delete, func, select
from sqlalchemy.orm import Session

from pennychest.core.database import get_db
from pennychest.mcp.models import AccessToken, OAuthClient, OAuthCode
from pennychest.mcp.tokens import TOKEN_PREFIX, WRITE_SCOPES, _hash

ACCESS_TOKEN_SECONDS = 60 * 60
CODE_SECONDS = 10 * 60
MAX_CLIENTS = 200
SCOPES_SUPPORTED = ["read", *WRITE_SCOPES]

router = APIRouter(tags=["oauth"])


def public_base(request: Request) -> str:
    """This server's public address, e.g. https://pennychest.example. Behind a proxy (Fly) the
    app itself is reached over http, so the scheme comes from X-Forwarded-Proto. Only the scheme
    is taken from it: forwarded client addresses aren't trusted."""
    base = str(request.base_url).rstrip("/")
    proto = request.headers.get("x-forwarded-proto", "").split(",")[0].strip()
    if proto in ("http", "https"):
        base = f"{proto}://{base.split('://', 1)[1]}"
    return base


def resource_url(request: Request) -> str:
    return f"{public_base(request)}/mcp"


def resource_metadata_url(request: Request) -> str:
    return f"{public_base(request)}/.well-known/oauth-protected-resource/mcp"


def _no_store(body: dict, status: int = 200) -> JSONResponse:
    return JSONResponse(body, status_code=status, headers={"Cache-Control": "no-store"})


def _oauth_error(error: str, description: str, status: int = 400) -> JSONResponse:
    return _no_store({"error": error, "error_description": description}, status)


# Discovery


@router.get("/.well-known/oauth-protected-resource")
@router.get("/.well-known/oauth-protected-resource/mcp")
def protected_resource_metadata(request: Request):
    return {
        "resource": resource_url(request),
        "authorization_servers": [public_base(request)],
        "scopes_supported": SCOPES_SUPPORTED,
        "bearer_methods_supported": ["header"],
        "resource_name": "PennyChest",
    }


@router.get("/.well-known/oauth-authorization-server")
def authorization_server_metadata(request: Request):
    base = public_base(request)
    return {
        "issuer": base,
        "authorization_endpoint": f"{base}/oauth/authorize",
        "token_endpoint": f"{base}/oauth/token",
        "registration_endpoint": f"{base}/oauth/register",
        "revocation_endpoint": f"{base}/oauth/revoke",
        "scopes_supported": SCOPES_SUPPORTED,
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": [
            "none",
            "client_secret_post",
            "client_secret_basic",
        ],
    }


# Dynamic client registration


def _valid_redirect(uri: str) -> bool:
    parts = urlsplit(uri)
    if parts.fragment or not parts.netloc:
        return False
    if parts.scheme == "https":
        return True
    # Plain http only back to the user's own machine (desktop and CLI clients)
    return parts.scheme == "http" and parts.hostname in ("localhost", "127.0.0.1", "::1")


class Registration(BaseModel):
    redirect_uris: list[str]
    client_name: str | None = None
    token_endpoint_auth_method: str | None = None
    grant_types: list[str] | None = None
    response_types: list[str] | None = None
    scope: str | None = None


@router.post("/oauth/register", status_code=201)
def register_client(body: Registration, db: Session = Depends(get_db)):
    if not body.redirect_uris or not all(_valid_redirect(u) for u in body.redirect_uris):
        return _oauth_error(
            "invalid_redirect_uri", "Redirect URIs must be https, or http to localhost."
        )
    method = body.token_endpoint_auth_method or "none"
    if method not in ("none", "client_secret_post", "client_secret_basic"):
        return _oauth_error("invalid_client_metadata", f"Unsupported auth method {method!r}.")

    # Registration is open, so keep it from growing without bound: clients that registered
    # but never connected are dropped after a day.
    day_ago = time.time() - 24 * 60 * 60
    connected = select(AccessToken.client_id).where(AccessToken.client_id.is_not(None))
    for stale in db.execute(
        select(OAuthClient).where(OAuthClient.client_id.not_in(connected))
    ).scalars():
        if stale.created_at and stale.created_at.timestamp() < day_ago:
            db.delete(stale)
    db.flush()
    if db.execute(select(func.count()).select_from(OAuthClient)).scalar_one() >= MAX_CLIENTS:
        return _oauth_error("temporarily_unavailable", "Too many registered clients.", 503)

    client_id = "pcc_" + secrets.token_urlsafe(16)
    secret = secrets.token_urlsafe(32) if method != "none" else None
    client = OAuthClient(
        client_id=client_id,
        client_secret_hash=_hash(secret) if secret else None,
        client_name=(body.client_name or "An MCP client").strip()[:80],
        redirect_uris=body.redirect_uris,
    )
    db.add(client)
    db.commit()
    response = {
        "client_id": client_id,
        "client_id_issued_at": int(time.time()),
        "client_name": client.client_name,
        "redirect_uris": body.redirect_uris,
        "token_endpoint_auth_method": method,
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
    }
    if secret:
        response.update(client_secret=secret, client_secret_expires_at=0)
    return _no_store(response, 201)


# The consent page (frontend /oauth/authorize) uses these, signed in as the user


def _client_for(db: Session, client_id: str, redirect_uri: str) -> OAuthClient:
    client = db.execute(
        select(OAuthClient).where(OAuthClient.client_id == client_id)
    ).scalar_one_or_none()
    if not client:
        raise HTTPException(status_code=400, detail="This app isn't registered with PennyChest.")
    if redirect_uri not in client.redirect_uris:
        raise HTTPException(status_code=400, detail="This app's return address doesn't match.")
    return client


@router.get("/api/oauth/authorize-info")
def authorize_info(client_id: str, redirect_uri: str, db: Session = Depends(get_db)):
    client = _client_for(db, client_id, redirect_uri)
    return {"client_name": client.client_name, "redirect_host": urlsplit(redirect_uri).netloc}


class Decision(BaseModel):
    client_id: str
    redirect_uri: str
    state: str | None = None
    code_challenge: str | None = None
    code_challenge_method: str | None = None
    response_type: str = "code"
    scopes: list[str] = []


def _redirect(request: Request, uri: str, params: dict) -> dict:
    params = {k: v for k, v in params.items() if v is not None}
    params["iss"] = public_base(request)
    separator = "&" if urlsplit(uri).query else "?"
    return {"redirect": f"{uri}{separator}{urlencode(params)}"}


@router.post("/api/oauth/approve")
def approve(body: Decision, request: Request, db: Session = Depends(get_db)):
    client = _client_for(db, body.client_id, body.redirect_uri)
    if body.response_type != "code":
        return _redirect(
            request, body.redirect_uri, {"error": "unsupported_response_type", "state": body.state}
        )
    if not body.code_challenge or body.code_challenge_method != "S256":
        return _redirect(
            request,
            body.redirect_uri,
            {
                "error": "invalid_request",
                "error_description": "PKCE (S256) is required.",
                "state": body.state,
            },
        )
    code = secrets.token_urlsafe(32)
    db.add(
        OAuthCode(
            code_hash=_hash(code),
            client_id=client.client_id,
            redirect_uri=body.redirect_uri,
            code_challenge=body.code_challenge,
            scopes=[s for s in WRITE_SCOPES if s in body.scopes],
            expires_at=int(time.time()) + CODE_SECONDS,
        )
    )
    db.commit()
    return _redirect(request, body.redirect_uri, {"code": code, "state": body.state})


@router.post("/api/oauth/deny")
def deny(body: Decision, request: Request, db: Session = Depends(get_db)):
    _client_for(db, body.client_id, body.redirect_uri)
    return _redirect(request, body.redirect_uri, {"error": "access_denied", "state": body.state})


# Tokens


def _authenticate_client(
    db: Session, client_id: str | None, client_secret: str | None, authorization: str | None
) -> OAuthClient | None:
    if authorization and authorization.lower().startswith("basic "):
        try:
            decoded = base64.b64decode(authorization[6:]).decode()
            client_id, client_secret = decoded.split(":", 1)
        except (ValueError, UnicodeDecodeError):
            return None
    if not client_id:
        return None
    client = db.execute(
        select(OAuthClient).where(OAuthClient.client_id == client_id)
    ).scalar_one_or_none()
    if not client:
        return None
    if client.client_secret_hash:
        if not client_secret or not secrets.compare_digest(
            _hash(client_secret), client.client_secret_hash
        ):
            return None
    return client


def _issue(db: Session, token: AccessToken) -> JSONResponse:
    access = TOKEN_PREFIX + secrets.token_urlsafe(32)
    refresh = "pcr_" + secrets.token_urlsafe(32)
    token.token_hash = _hash(access)
    token.prefix = access[:10]
    token.refresh_hash = _hash(refresh)
    token.expires_at = int(time.time()) + ACCESS_TOKEN_SECONDS
    db.commit()
    return _no_store(
        {
            "access_token": access,
            "token_type": "Bearer",
            "expires_in": ACCESS_TOKEN_SECONDS,
            "refresh_token": refresh,
            "scope": " ".join(["read", *token.scopes]),
        }
    )


def _pkce_matches(verifier: str, challenge: str) -> bool:
    digest = hashlib.sha256(verifier.encode()).digest()
    expected = base64.urlsafe_b64encode(digest).rstrip(b"=").decode()
    return secrets.compare_digest(expected, challenge)


@router.post("/oauth/token")
def token_endpoint(
    grant_type: str = Form(...),
    code: str | None = Form(None),
    redirect_uri: str | None = Form(None),
    code_verifier: str | None = Form(None),
    refresh_token: str | None = Form(None),
    client_id: str | None = Form(None),
    client_secret: str | None = Form(None),
    authorization: str | None = Header(None),
    db: Session = Depends(get_db),
):
    client = _authenticate_client(db, client_id, client_secret, authorization)
    if not client:
        return _oauth_error("invalid_client", "Unknown client or wrong secret.", 401)

    if grant_type == "authorization_code":
        if not code or not code_verifier:
            return _oauth_error("invalid_request", "code and code_verifier are required.")
        grant = db.execute(
            select(OAuthCode).where(OAuthCode.code_hash == _hash(code))
        ).scalar_one_or_none()
        if (
            not grant
            or grant.used
            or grant.client_id != client.client_id
            or grant.expires_at < time.time()
            or (redirect_uri and redirect_uri != grant.redirect_uri)
            or not _pkce_matches(code_verifier, grant.code_challenge)
        ):
            return _oauth_error("invalid_grant", "The code is invalid, expired or already used.")
        grant.used = True
        token = AccessToken(
            name=client.client_name,
            token_hash="",
            prefix="",
            scopes=grant.scopes,
            client_id=client.client_id,
        )
        # Codes are single use and short-lived; tidy up the spent ones
        db.execute(delete(OAuthCode).where(OAuthCode.expires_at < time.time()))
        db.add(token)
        return _issue(db, token)

    if grant_type == "refresh_token":
        if not refresh_token:
            return _oauth_error("invalid_request", "refresh_token is required.")
        token = db.execute(
            select(AccessToken).where(AccessToken.refresh_hash == _hash(refresh_token))
        ).scalar_one_or_none()
        if not token or token.client_id != client.client_id:
            return _oauth_error("invalid_grant", "The refresh token is invalid or was revoked.")
        return _issue(db, token)

    return _oauth_error("unsupported_grant_type", f"Unsupported grant type {grant_type!r}.")


@router.post("/oauth/revoke")
def revoke_endpoint(
    token: str = Form(...),
    client_id: str | None = Form(None),
    client_secret: str | None = Form(None),
    authorization: str | None = Header(None),
    db: Session = Depends(get_db),
):
    client = _authenticate_client(db, client_id, client_secret, authorization)
    if not client:
        return _oauth_error("invalid_client", "Unknown client or wrong secret.", 401)
    hashed = _hash(token)
    found = db.execute(
        select(AccessToken).where(
            AccessToken.client_id == client.client_id,
            (AccessToken.token_hash == hashed) | (AccessToken.refresh_hash == hashed),
        )
    ).scalar_one_or_none()
    if found:
        db.delete(found)
        db.commit()
    # RFC 7009: the same answer whether or not the token existed
    return _no_store({})
