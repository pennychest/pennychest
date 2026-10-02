from sqlalchemy import (
    CheckConstraint,
    Column,
    Date,
    DateTime,
    Float,
    ForeignKey,
    Integer,
    Numeric,
    String,
    func,
)
from sqlalchemy.orm import relationship

from pennychest.accounts.models import Base
from pennychest.core.types import JSONType


class Transaction(Base):
    __tablename__ = "transactions"

    id = Column(Integer, primary_key=True)
    date = Column(Date, nullable=False)
    description = Column(String, nullable=False)
    status = Column(
        String,
        nullable=False,
        server_default="pending",
    )
    import_batch_id = Column(
        Integer, ForeignKey("import_batches.id", ondelete="SET NULL"), nullable=True
    )
    created_at = Column(DateTime(timezone=True), nullable=False, server_default=func.now())
    currency = Column(String, nullable=False, server_default="GBP")
    import_metadata = Column(JSONType, nullable=True)
    transfer_peer_id = Column(
        Integer,
        ForeignKey("transactions.id", ondelete="SET NULL"),
        nullable=True,
    )
    # Set at import when this looks like a transaction already in the ledger, until the user
    # deletes one of them or says it isn't a duplicate.
    duplicate_of_id = Column(
        Integer,
        ForeignKey("transactions.id", ondelete="SET NULL"),
        nullable=True,
    )
    # How unusual this charge is for its merchant and category, from 0 (normal) to 1, as the
    # model chosen for the "insights" task judged it. None until it has been scored.
    unusual_score = Column(Float, nullable=True)

    __table_args__ = (CheckConstraint("status IN ('pending', 'confirmed')", name="ck_status"),)

    postings = relationship("Posting", back_populates="transaction", cascade="all, delete-orphan")
    import_batch = relationship("ImportBatch")
    transfer_peer = relationship(
        "Transaction",
        foreign_keys=[transfer_peer_id],
        primaryjoin="Transaction.transfer_peer_id == Transaction.id",
        uselist=False,
        post_update=True,
    )
    duplicate_of = relationship(
        "Transaction",
        foreign_keys=[duplicate_of_id],
        primaryjoin="Transaction.duplicate_of_id == Transaction.id",
        remote_side=[id],
        uselist=False,
    )


class Posting(Base):
    __tablename__ = "postings"

    id = Column(Integer, primary_key=True)
    transaction_id = Column(Integer, ForeignKey("transactions.id"), nullable=False)
    account_id = Column(Integer, ForeignKey("accounts.id"), nullable=False)
    amount = Column(Numeric(19, 4), nullable=False)
    categorised_by_id = Column(
        Integer, ForeignKey("categorisation_sources.id"), nullable=False
    )
    rule_id = Column(
        Integer, ForeignKey("rules.id", ondelete="SET NULL"), nullable=True
    )

    transaction = relationship("Transaction", back_populates="postings")
    account = relationship("Account")
    categorised_by = relationship("CategorisationSource")
    rule = relationship("Rule")
