from sqlalchemy import (
    Boolean,
    Column,
    Date,
    DateTime,
    Float,
    ForeignKey,
    Integer,
    Numeric,
    String,
    false,
    func,
)
from sqlalchemy.orm import relationship

from pennychest.accounts.models import Base


class WalletCard(Base):
    """Pairs a card name reported by the phone wallet with a ledger account."""

    __tablename__ = "wallet_cards"

    id = Column(Integer, primary_key=True)
    card_name = Column(String, nullable=False, unique=True)
    account_id = Column(Integer, ForeignKey("accounts.id", ondelete="CASCADE"), nullable=False)
    created_at = Column(DateTime(timezone=True), nullable=False, server_default=func.now())

    account = relationship("Account")


class CardTap(Base):
    """A card payment reported in real time, before it appears on a statement.

    Taps live outside the ledger. Once a statement containing the payment is
    imported, the tap is reconciled by pointing it at the imported transaction.
    """

    __tablename__ = "card_taps"

    id = Column(Integer, primary_key=True)
    tapped_on = Column(Date, nullable=False)
    merchant = Column(String, nullable=False)
    # Positive for a purchase, negative for a refund
    amount = Column(Numeric(19, 4), nullable=False)
    currency = Column(String, nullable=False, server_default="GBP")
    card_name = Column(String, nullable=True)
    transaction_id = Column(
        Integer, ForeignKey("transactions.id", ondelete="SET NULL"), nullable=True, unique=True
    )
    dismissed = Column(Boolean, nullable=False, server_default=false())
    created_at = Column(DateTime(timezone=True), nullable=False, server_default=func.now())
    # The category an AI model suggested when the tap arrived, and how sure it was
    # (decision models such as Jev report confidence; LLMs don't).
    ai_account_id = Column(Integer, ForeignKey("accounts.id", ondelete="SET NULL"), nullable=True)
    ai_confidence = Column(Float, nullable=True)

    transaction = relationship("Transaction")
    ai_account = relationship("Account")
