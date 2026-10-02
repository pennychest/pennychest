import enum

from sqlalchemy import Column, Enum, ForeignKey, Integer, String
from sqlalchemy.orm import DeclarativeBase, relationship


class Base(DeclarativeBase):
    pass


class AccountType(str, enum.Enum):
    ASSET = "asset"
    LIABILITY = "liability"
    INCOME = "income"
    EXPENSE = "expense"
    EQUITY = "equity"


class Account(Base):
    __tablename__ = "accounts"

    id = Column(Integer, primary_key=True)
    name = Column(String, nullable=False)
    full_path = Column(String, nullable=False, unique=True)
    parent_id = Column(Integer, ForeignKey("accounts.id"), nullable=True)
    type = Column(Enum(AccountType, name="account_type", create_constraint=True), nullable=False)
    currency = Column(String, nullable=False, server_default="GBP")
    bank_identifier = Column(String, nullable=True)
    icon = Column(String, nullable=True)

    parent = relationship("Account", remote_side=[id], back_populates="children")
    children = relationship("Account", back_populates="parent")
