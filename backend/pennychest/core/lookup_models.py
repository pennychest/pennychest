from sqlalchemy import Column, Integer, String

from pennychest.accounts.models import Base


class BudgetPeriod(Base):
    __tablename__ = "budget_periods"
    id = Column(Integer, primary_key=True)
    name = Column(String, nullable=False, unique=True)


class MatchType(Base):
    __tablename__ = "match_types"
    id = Column(Integer, primary_key=True)
    name = Column(String, nullable=False, unique=True)


class CategorisationSource(Base):
    __tablename__ = "categorisation_sources"
    id = Column(Integer, primary_key=True)
    name = Column(String, nullable=False, unique=True)


class ImportSourceType(Base):
    __tablename__ = "import_source_types"
    id = Column(Integer, primary_key=True)
    name = Column(String, nullable=False, unique=True)
