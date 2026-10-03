from sqlalchemy import Column, DateTime, ForeignKey, Integer, String, Text, func

from pennychest.accounts.models import Base
from pennychest.core.types import JSONType


class ActionChange(Base):
    """A change made through an action, with what's needed to undo it."""

    __tablename__ = "action_changes"

    id = Column(Integer, primary_key=True)
    created_at = Column(DateTime(timezone=True), nullable=False, server_default=func.now())
    action = Column(String, nullable=False)
    arguments = Column(JSONType, nullable=True)
    summary = Column(Text, nullable=False)
    undo = Column(JSONType, nullable=False)
    source = Column(String, nullable=False)  # "chat", "api" or "mcp"
    conversation_id = Column(
        Integer, ForeignKey("chat_conversations.id", ondelete="SET NULL"), nullable=True
    )
    chat_message_id = Column(
        Integer, ForeignKey("chat_messages.id", ondelete="SET NULL"), nullable=True
    )
    undone_at = Column(DateTime(timezone=True), nullable=True)
