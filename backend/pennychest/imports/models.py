from sqlalchemy import Column, Date, DateTime, ForeignKey, Integer, Numeric, String, func
from sqlalchemy.orm import relationship

from pennychest.accounts.models import Base


class ImportBatch(Base):
    __tablename__ = "import_batches"

    id = Column(Integer, primary_key=True)
    importer_name = Column(String, nullable=False)
    source_type_id = Column(Integer, ForeignKey("import_source_types.id"), nullable=False)
    file_path = Column(String, nullable=True)
    file_hash = Column(String, nullable=True)
    file_name = Column(String, nullable=True)
    account_id = Column(Integer, ForeignKey("accounts.id"), nullable=True)
    imported_at = Column(DateTime(timezone=True), nullable=False, server_default=func.now())
    # Statement-level summary, populated by importers that can extract it
    # (currently only HSBC). Used for the opening-balance recommendation flow
    # on upload and for the future statement-coverage view.
    period_start = Column(Date, nullable=True)
    period_end = Column(Date, nullable=True)
    opening_balance = Column(Numeric(19, 4), nullable=True)
    closing_balance = Column(Numeric(19, 4), nullable=True)

    source_type = relationship("ImportSourceType")
    account = relationship("Account")
    raw_rows = relationship("RawImportRow", back_populates="batch")


class RawImportRow(Base):
    __tablename__ = "raw_import_rows"

    id = Column(Integer, primary_key=True)
    batch_id = Column(Integer, ForeignKey("import_batches.id"), nullable=False)
    raw_content = Column(String, nullable=False)
    line_number = Column(Integer, nullable=True)
    transaction_id = Column(Integer, ForeignKey("transactions.id"), nullable=True)

    batch = relationship("ImportBatch", back_populates="raw_rows")
    transaction = relationship("Transaction")
