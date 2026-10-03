from sqlalchemy import (
    BigInteger,
    Boolean,
    Column,
    DateTime,
    ForeignKey,
    Integer,
    String,
    false,
    func,
)

from pennychest.accounts.models import Base
from pennychest.core.types import JSONType


class AccessToken(Base):
    """A personal access token for the MCP server, scoped to what its agent may change."""

    __tablename__ = "access_tokens"

    id = Column(Integer, primary_key=True)
    name = Column(String, nullable=False)
    # SHA-256 of the token; the token itself is shown once, when it's created
    token_hash = Column(String, nullable=False, unique=True)
    # The start of the token, so the user can tell their tokens apart
    prefix = Column(String, nullable=False)
    # Write scopes on top of read (see pennychest.actions.registry.Scope)
    scopes = Column(JSONType, nullable=False)
    created_at = Column(DateTime(timezone=True), nullable=False, server_default=func.now())
    last_used_at = Column(DateTime(timezone=True), nullable=True)
    # Set for tokens issued to an OAuth connector (claude.ai, ChatGPT): the access token
    # expires and is renewed with the refresh token, which rotates on every use.
    client_id = Column(
        String, ForeignKey("oauth_clients.client_id", ondelete="CASCADE"), nullable=True
    )
    refresh_hash = Column(String, nullable=True, unique=True)
    # Unix seconds, which compare the same way on SQLite and Postgres
    expires_at = Column(BigInteger, nullable=True)


class OAuthClient(Base):
    """A connector that registered itself (RFC 7591 dynamic client registration)."""

    __tablename__ = "oauth_clients"

    id = Column(Integer, primary_key=True)
    client_id = Column(String, nullable=False, unique=True)
    # Only for clients that asked to authenticate with a secret; public clients rely on PKCE
    client_secret_hash = Column(String, nullable=True)
    client_name = Column(String, nullable=False)
    redirect_uris = Column(JSONType, nullable=False)
    created_at = Column(DateTime(timezone=True), nullable=False, server_default=func.now())


class OAuthCode(Base):
    """A one-time authorization code, issued when the user approves a connector."""

    __tablename__ = "oauth_codes"

    id = Column(Integer, primary_key=True)
    code_hash = Column(String, nullable=False, unique=True)
    client_id = Column(
        String, ForeignKey("oauth_clients.client_id", ondelete="CASCADE"), nullable=False
    )
    redirect_uri = Column(String, nullable=False)
    code_challenge = Column(String, nullable=False)
    scopes = Column(JSONType, nullable=False)
    expires_at = Column(BigInteger, nullable=False)
    used = Column(Boolean, nullable=False, server_default=false())
