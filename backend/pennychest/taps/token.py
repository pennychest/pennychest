import secrets

from sqlalchemy.orm import Session

from pennychest.settings.models import AppSetting

# Stored readable (not hashed) so Settings can show it again when adding another
# phone. It only allows recording taps, and reading it requires signing in.
TAP_TOKEN_KEY = "tap_token"


def get_tap_token(db: Session) -> str | None:
    setting = db.get(AppSetting, TAP_TOKEN_KEY)
    return setting.value if setting else None


def regenerate_tap_token(db: Session) -> str:
    token = secrets.token_urlsafe(32)
    setting = db.get(AppSetting, TAP_TOKEN_KEY)
    if setting:
        setting.value = token
    else:
        db.add(AppSetting(key=TAP_TOKEN_KEY, value=token))
    db.commit()
    return token


def clear_tap_token(db: Session) -> None:
    setting = db.get(AppSetting, TAP_TOKEN_KEY)
    if setting:
        db.delete(setting)
        db.commit()
