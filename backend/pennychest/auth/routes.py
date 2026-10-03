from fastapi import APIRouter, Depends, HTTPException, Request, Response
from pydantic import BaseModel
from sqlalchemy.orm import Session

from pennychest.auth.service import (
    MIN_PASSWORD_LENGTH,
    SESSION_TTL_SECONDS,
    create_session,
    end_other_sessions,
    end_session,
    get_password_hash,
    is_valid_session,
    login_throttle,
    set_password,
    verify_password,
)
from pennychest.core.database import get_db

COOKIE_NAME = "pennychest_session"

# Reachable without signing in. Everything else under /api/ needs a session.
PUBLIC_PATHS = {
    "/api/health",
    "/api/auth/status",
    "/api/auth/setup",
    "/api/auth/login",
    "/api/auth/logout",
}
# Called by the phone wallet shortcut, which authenticates with the tap token from Settings.
TOKEN_AUTH_ROUTES = {("POST", "/api/taps")}

router = APIRouter(prefix="/api/auth", tags=["auth"])


def require_session(request: Request, db: Session = Depends(get_db)) -> None:
    path = request.url.path
    if not path.startswith("/api/") or path in PUBLIC_PATHS:
        return
    if (request.method, path) in TOKEN_AUTH_ROUTES:
        return
    if not is_valid_session(db, request.cookies.get(COOKIE_NAME)):
        raise HTTPException(status_code=401, detail="Not signed in")


class PasswordRequest(BaseModel):
    password: str


class ChangePasswordRequest(BaseModel):
    current_password: str
    new_password: str


class AuthStatus(BaseModel):
    setup_required: bool
    authenticated: bool


def _check_strength(password: str) -> None:
    if len(password) < MIN_PASSWORD_LENGTH:
        raise HTTPException(
            status_code=400,
            detail=f"Password must be at least {MIN_PASSWORD_LENGTH} characters",
        )


def _sign_in(request: Request, response: Response, db: Session) -> None:
    secure = (
        request.url.scheme == "https"
        or request.headers.get("x-forwarded-proto", "").split(",")[0].strip() == "https"
    )
    response.set_cookie(
        COOKIE_NAME,
        create_session(db),
        max_age=SESSION_TTL_SECONDS,
        httponly=True,
        secure=secure,
        samesite="lax",
    )


def _client_key(request: Request) -> str:
    # Behind a reverse proxy this is the proxy's address, so the limit becomes global.
    return request.client.host if request.client else "unknown"


@router.get("/status", response_model=AuthStatus)
def status(request: Request, db: Session = Depends(get_db)):
    return AuthStatus(
        setup_required=get_password_hash(db) is None,
        authenticated=is_valid_session(db, request.cookies.get(COOKIE_NAME)),
    )


@router.post("/setup", status_code=204)
def setup(
    body: PasswordRequest, request: Request, response: Response, db: Session = Depends(get_db)
):
    if get_password_hash(db) is not None:
        raise HTTPException(status_code=409, detail="A password has already been set")
    _check_strength(body.password)
    set_password(db, body.password)
    _sign_in(request, response, db)


@router.post("/login", status_code=204)
def login(
    body: PasswordRequest, request: Request, response: Response, db: Session = Depends(get_db)
):
    key = _client_key(request)
    if login_throttle.is_blocked(key):
        raise HTTPException(status_code=429, detail="Too many failed attempts. Try again later.")
    stored = get_password_hash(db)
    if stored is None:
        raise HTTPException(status_code=409, detail="No password has been set yet")
    if not verify_password(body.password, stored):
        login_throttle.record_failure(key)
        raise HTTPException(status_code=401, detail="Incorrect password")
    login_throttle.reset(key)
    _sign_in(request, response, db)


@router.post("/logout", status_code=204)
def logout(request: Request, response: Response, db: Session = Depends(get_db)):
    end_session(db, request.cookies.get(COOKIE_NAME))
    response.delete_cookie(COOKIE_NAME)


@router.post("/password", status_code=204)
def change_password(body: ChangePasswordRequest, request: Request, db: Session = Depends(get_db)):
    stored = get_password_hash(db)
    if stored is None or not verify_password(body.current_password, stored):
        raise HTTPException(status_code=400, detail="Current password is incorrect")
    _check_strength(body.new_password)
    set_password(db, body.new_password)
    # Sign out every other device; this one keeps its session.
    end_other_sessions(db, request.cookies[COOKIE_NAME])
