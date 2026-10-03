from sqlalchemy import CheckConstraint, Column, DateTime, Float, Integer, String, Text, func

from pennychest.accounts.models import Base
from pennychest.core.types import JSONType


class AIRequestLog(Base):
    __tablename__ = "ai_request_logs"

    id = Column(Integer, primary_key=True)
    created_at = Column(DateTime(timezone=True), nullable=False, server_default=func.now())
    provider = Column(Text, nullable=False)
    operation = Column(Text, nullable=False)
    request_data = Column(JSONType, nullable=True)
    response_data = Column(JSONType, nullable=True)
    duration_ms = Column(Integer, nullable=True)
    error = Column(Text, nullable=True)


class MerchantLabel(Base):
    """What kind of payment a recurring merchant is, as the model judged it. Keyed by
    actions.insights.merchant_key, so it covers every charge from that merchant."""

    __tablename__ = "merchant_labels"

    merchant_key = Column(String, primary_key=True)
    kind = Column(String, nullable=False)  # subscription, bill or other
    confidence = Column(Float, nullable=True)
    updated_at = Column(DateTime(timezone=True), nullable=False, server_default=func.now())

    __table_args__ = (
        CheckConstraint("kind IN ('subscription', 'bill', 'other')", name="ck_merchant_kind"),
    )
