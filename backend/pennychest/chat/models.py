from sqlalchemy import Boolean, Column, DateTime, ForeignKey, Integer, String, Text, false, func
from sqlalchemy.orm import relationship

from pennychest.accounts.models import Base
from pennychest.core.types import JSONType


class ChatConversation(Base):
    __tablename__ = "chat_conversations"

    id = Column(Integer, primary_key=True)
    title = Column(String, nullable=False)
    created_at = Column(DateTime(timezone=True), nullable=False, server_default=func.now())
    updated_at = Column(DateTime(timezone=True), nullable=False, server_default=func.now())

    messages = relationship(
        "ChatMessage",
        back_populates="conversation",
        order_by="ChatMessage.id",
        cascade="all, delete-orphan",
        passive_deletes=True,
    )


class ChatMessage(Base):
    """One entry in a conversation: the user's message, a model turn, or a tool result.

    Model turns keep `raw`, the message exactly as the provider returned it, so it can be
    replayed unchanged to that provider; the other columns are the provider-neutral form."""

    __tablename__ = "chat_messages"

    id = Column(Integer, primary_key=True)
    conversation_id = Column(
        Integer,
        ForeignKey("chat_conversations.id", ondelete="CASCADE"),
        nullable=False,
        index=True,
    )
    role = Column(String, nullable=False)  # "user", "assistant" or "tool"
    content = Column(Text, nullable=True)
    tool_calls = Column(JSONType, nullable=True)
    tool_call_id = Column(String, nullable=True)
    tool_name = Column(String, nullable=True)
    is_error = Column(Boolean, nullable=False, server_default=false())
    raw = Column(JSONType, nullable=True)
    raw_provider = Column(String, nullable=True)
    created_at = Column(DateTime(timezone=True), nullable=False, server_default=func.now())

    conversation = relationship("ChatConversation", back_populates="messages")
