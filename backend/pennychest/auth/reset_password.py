"""Clear the password and sign out everywhere; the next visit shows the setup screen.

Usage (inside the container): python -m pennychest.auth.reset_password
"""
from sqlalchemy import delete

from pennychest.auth.models import AuthSession
from pennychest.auth.service import PASSWORD_KEY
from pennychest.core.database import SessionLocal
from pennychest.settings.models import AppSetting


def main() -> None:
    with SessionLocal() as db:
        db.execute(delete(AppSetting).where(AppSetting.key == PASSWORD_KEY))
        db.execute(delete(AuthSession))
        db.commit()
    print("Password cleared. Open PennyChest to choose a new one.")


if __name__ == "__main__":
    main()
