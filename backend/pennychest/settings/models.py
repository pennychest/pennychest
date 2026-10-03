from sqlalchemy import Column, String, Text

from pennychest.accounts.models import Base


class AppSetting(Base):
    __tablename__ = "app_settings"

    key = Column(String, primary_key=True)
    value = Column(Text, nullable=True)
