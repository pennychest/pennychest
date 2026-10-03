from sqlalchemy import (
    Column,
    DateTime,
    ForeignKey,
    Integer,
    Numeric,
    UniqueConstraint,
    func,
)
from sqlalchemy.orm import relationship

from pennychest.accounts.models import Base


class Budget(Base):
    """A spending limit for a category, covering it and all its subcategories."""

    __tablename__ = "budgets"
    __table_args__ = (
        UniqueConstraint("account_id", "period_id", name="uq_budgets_account_period"),
    )

    id = Column(Integer, primary_key=True)
    account_id = Column(Integer, ForeignKey("accounts.id", ondelete="CASCADE"), nullable=False)
    period_id = Column(Integer, ForeignKey("budget_periods.id"), nullable=False)
    amount = Column(Numeric(19, 4), nullable=False)
    created_at = Column(DateTime(timezone=True), nullable=False, server_default=func.now())

    account = relationship("Account")
    period = relationship("BudgetPeriod")
