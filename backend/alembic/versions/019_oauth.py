"""OAuth for MCP connectors: registered clients, authorization codes, expiring tokens

Revision ID: 019
Revises: 018
Create Date: 2026-10-01

"""

from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa
from sqlalchemy.dialects.postgresql import JSONB

JSON_TYPE = sa.JSON().with_variant(JSONB(), "postgresql")

revision: str = "019"
down_revision: Union[str, None] = "018"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.create_table(
        "oauth_clients",
        sa.Column("id", sa.Integer, primary_key=True),
        sa.Column("client_id", sa.String, nullable=False, unique=True),
        sa.Column("client_secret_hash", sa.String, nullable=True),
        sa.Column("client_name", sa.String, nullable=False),
        sa.Column("redirect_uris", JSON_TYPE, nullable=False),
        sa.Column(
            "created_at", sa.DateTime(timezone=True), nullable=False, server_default=sa.func.now()
        ),
    )
    op.create_table(
        "oauth_codes",
        sa.Column("id", sa.Integer, primary_key=True),
        sa.Column("code_hash", sa.String, nullable=False, unique=True),
        sa.Column(
            "client_id",
            sa.String,
            sa.ForeignKey("oauth_clients.client_id", ondelete="CASCADE"),
            nullable=False,
        ),
        sa.Column("redirect_uri", sa.String, nullable=False),
        sa.Column("code_challenge", sa.String, nullable=False),
        sa.Column("scopes", JSON_TYPE, nullable=False),
        sa.Column("expires_at", sa.BigInteger, nullable=False),
        sa.Column("used", sa.Boolean, nullable=False, server_default=sa.false()),
    )
    with op.batch_alter_table("access_tokens") as batch:
        batch.add_column(
            sa.Column(
                "client_id",
                sa.String,
                sa.ForeignKey(
                    "oauth_clients.client_id",
                    ondelete="CASCADE",
                    name="access_tokens_client_id_fkey",
                ),
                nullable=True,
            )
        )
        batch.add_column(sa.Column("refresh_hash", sa.String, nullable=True))
        batch.add_column(sa.Column("expires_at", sa.BigInteger, nullable=True))
        batch.create_unique_constraint("access_tokens_refresh_hash_key", ["refresh_hash"])


def downgrade() -> None:
    with op.batch_alter_table("access_tokens") as batch:
        batch.drop_constraint("access_tokens_refresh_hash_key", type_="unique")
        batch.drop_column("expires_at")
        batch.drop_column("refresh_hash")
        batch.drop_column("client_id")
    op.drop_table("oauth_codes")
    op.drop_table("oauth_clients")
