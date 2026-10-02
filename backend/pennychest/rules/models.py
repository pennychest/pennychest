from sqlalchemy import Column, ForeignKey, Integer, String

from pennychest.accounts.models import Base


class Rule(Base):
    __tablename__ = "rules"

    id = Column(Integer, primary_key=True)
    pattern = Column(String, nullable=False)
    match_type_id = Column(Integer, ForeignKey("match_types.id"), nullable=False)
    target_account_id = Column(Integer, ForeignKey("accounts.id"), nullable=False)
    priority = Column(Integer, nullable=False, server_default="0")
    source = Column(String, nullable=True, server_default="manual")
    description = Column(String, nullable=True)
