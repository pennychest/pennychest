from sqlalchemy import BigInteger, Column, DateTime, Integer, String, func

from pennychest.accounts.models import Base


class AuthSession(Base):
    __tablename__ = "auth_sessions"

    id = Column(Integer, primary_key=True)
    # SHA-256 of the cookie token, so a leaked database can't be used to sign in
    token_hash = Column(String, nullable=False, unique=True)
    created_at = Column(DateTime(timezone=True), nullable=False, server_default=func.now())
    # Unix seconds, which compare the same way on SQLite and Postgres
    expires_at = Column(BigInteger, nullable=False)
