import hashlib
import hmac
import secrets
import threading
import time
from collections import defaultdict, deque

from sqlalchemy import delete, select
from sqlalchemy.orm import Session

from pennychest.auth.models import AuthSession
from pennychest.settings.models import AppSetting

PASSWORD_KEY = "auth_password_hash"
MIN_PASSWORD_LENGTH = 8
SESSION_TTL_SECONDS = 30 * 24 * 3600

_SCRYPT_N, _SCRYPT_R, _SCRYPT_P = 2**14, 8, 1


def hash_password(password: str) -> str:
    salt = secrets.token_bytes(16)
    digest = hashlib.scrypt(
        password.encode(), salt=salt, n=_SCRYPT_N, r=_SCRYPT_R, p=_SCRYPT_P, dklen=32
    )
    return f"scrypt${_SCRYPT_N}${_SCRYPT_R}${_SCRYPT_P}${salt.hex()}${digest.hex()}"


def verify_password(password: str, stored: str) -> bool:
    try:
        _, n, r, p, salt, expected = stored.split("$")
        digest = hashlib.scrypt(
            password.encode(), salt=bytes.fromhex(salt), n=int(n), r=int(r), p=int(p), dklen=32
        )
    except ValueError:
        return False
    return hmac.compare_digest(digest.hex(), expected)


def get_password_hash(db: Session) -> str | None:
    setting = db.get(AppSetting, PASSWORD_KEY)
    return setting.value if setting else None


def set_password(db: Session, password: str) -> None:
    setting = db.get(AppSetting, PASSWORD_KEY)
    if setting:
        setting.value = hash_password(password)
    else:
        db.add(AppSetting(key=PASSWORD_KEY, value=hash_password(password)))
    db.commit()


def _hash_token(token: str) -> str:
    return hashlib.sha256(token.encode()).hexdigest()


def create_session(db: Session) -> str:
    now = int(time.time())
    db.execute(delete(AuthSession).where(AuthSession.expires_at <= now))
    token = secrets.token_urlsafe(32)
    db.add(AuthSession(token_hash=_hash_token(token), expires_at=now + SESSION_TTL_SECONDS))
    db.commit()
    return token


def is_valid_session(db: Session, token: str | None) -> bool:
    if not token:
        return False
    expires_at = db.execute(
        select(AuthSession.expires_at).where(AuthSession.token_hash == _hash_token(token))
    ).scalar_one_or_none()
    return expires_at is not None and expires_at > time.time()


def end_session(db: Session, token: str | None) -> None:
    if token:
        db.execute(delete(AuthSession).where(AuthSession.token_hash == _hash_token(token)))
        db.commit()


def end_other_sessions(db: Session, keep_token: str) -> None:
    db.execute(delete(AuthSession).where(AuthSession.token_hash != _hash_token(keep_token)))
    db.commit()


class LoginThrottle:
    """Allows a limited number of failed logins per client within a sliding window."""

    def __init__(self, max_failures: int = 10, window_seconds: int = 15 * 60):
        self.max_failures = max_failures
        self.window_seconds = window_seconds
        self._failures: dict[str, deque[float]] = defaultdict(deque)
        self._lock = threading.Lock()

    def _prune(self, key: str, now: float) -> deque[float]:
        failures = self._failures[key]
        while failures and failures[0] <= now - self.window_seconds:
            failures.popleft()
        return failures

    def is_blocked(self, key: str) -> bool:
        with self._lock:
            return len(self._prune(key, time.monotonic())) >= self.max_failures

    def record_failure(self, key: str) -> None:
        with self._lock:
            now = time.monotonic()
            self._prune(key, now).append(now)

    def reset(self, key: str | None = None) -> None:
        with self._lock:
            if key is None:
                self._failures.clear()
            else:
                self._failures.pop(key, None)


login_throttle = LoginThrottle()
