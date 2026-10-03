"""The chat loop: send the conversation to the chosen model, run the actions it asks for, feed
the results back, and repeat until it answers."""

import json
import time
from collections.abc import Iterable, Iterator
from dataclasses import replace
from datetime import UTC, date, datetime

from sqlalchemy import select
from sqlalchemy.orm import Session

from pennychest.actions.catalog import ACTIONS, ActionError, Scope, describe_actions, run_action
from pennychest.actions.models import ActionChange
from pennychest.ai.check_actions import check_action, checking_enabled
from pennychest.ai.models import AIRequestLog
from pennychest.ai.providers import ChatReply, ProviderError, ToolCall, resolve_task
from pennychest.chat.models import ChatConversation, ChatMessage

MAX_ROUNDS = 8
TITLE_LENGTH = 60

LAST_ROUND = """

You have used up your lookups for this message. Answer now from the results you already \
have, without calling any more tools, and say briefly if anything is missing."""


def _permissions(scopes: Iterable[str]) -> str:
    allowed = set(scopes)
    can = []
    if "organise" in allowed:
        can.append("create and rename categories, create rules and set budgets")
    if "transactions" in allowed:
        can.append("add, edit, recategorise, review and delete transactions")
    if not can:
        return """You can look things up but can't change anything. If the user asks for a \
change, explain that they can let you make changes in Settings > AI Integration > Chat."""
    cannot = [
        text
        for scope, text in (
            ("organise", "change categories, rules or budgets"),
            ("transactions", "change transactions"),
        )
        if scope not in allowed
    ]
    text = f"""When the user asks, you can {" and ".join(can)}. Do what they ask without asking \
for confirmation, then say exactly what you changed; they can undo any change from the chat. \
Look up the details you need (IDs, category paths) first rather than guessing, and don't make \
changes they didn't ask for."""
    if cannot:
        text += f""" You can't {" or ".join(cannot)}; if they ask, explain that they can allow \
it in Settings > AI Integration > Chat."""
    return text


def system_prompt(today: date, scopes: Iterable[str] = ()) -> str:
    return f"""You are the assistant inside PennyChest, a personal finance app. You help the user \
understand and manage their own money: spending, income, categories, budgets, rules and card \
payments.

Use the tools to look things up. When you need several lookups that don't depend on each \
other, ask for them all at once rather than one at a time. Prefer the broadest tool that \
answers the question: savings_opportunities for where they could save or cut back, \
recurring_payments for subscriptions and bills, compare_periods for what has changed. \
For open-ended questions where you need to see the detail, transactions_digest gives you a \
whole period in one call.

Every figure you give must come from a tool result; never estimate or do your own sums over \
transactions when a tool can total them. If a tool reports \
an error, fix the call and try again, or explain what's missing. Amounts are in pounds (GBP) \
unless a result says otherwise; spending is positive and refunds reduce it. Mention when \
totals include unreviewed transactions.

Answer concisely in plain English. Use short lists or small Markdown tables when they make \
figures easier to read. If the question is ambiguous (for example which period they mean), \
make a sensible assumption and say what you assumed.

{_permissions(scopes)}

Today is {today.strftime("%A %-d %B %Y")}."""


def _log(
    db: Session,
    label: str,
    request: dict | None,
    response: dict | None,
    started: float,
    error: str | None,
) -> None:
    db.add(
        AIRequestLog(
            provider=label,
            operation="chat",
            request_data=request,
            response_data=response,
            duration_ms=int((time.monotonic() - started) * 1000),
            error=error,
        )
    )


def history(conversation: ChatConversation) -> list[dict]:
    """The conversation in the provider-neutral form chat_turn expects."""
    entries = []
    for m in conversation.messages:
        if m.role == "user":
            entries.append({"role": "user", "content": m.content or ""})
        elif m.role == "assistant":
            entries.append(
                {
                    "role": "assistant",
                    "content": m.content or "",
                    "tool_calls": m.tool_calls or [],
                    "raw": m.raw,
                    "raw_provider": m.raw_provider,
                }
            )
        else:
            entries.append(
                {
                    "role": "tool",
                    "tool_call_id": m.tool_call_id,
                    "name": m.tool_name,
                    "content": m.content or "",
                    "is_error": m.is_error,
                }
            )
    return entries


def start_conversation(db: Session, first_message: str) -> ChatConversation:
    title = " ".join(first_message.split())
    if len(title) > TITLE_LENGTH:
        title = title[: TITLE_LENGTH - 1].rstrip() + "…"
    conversation = ChatConversation(title=title)
    db.add(conversation)
    db.commit()
    return conversation


def _add(db: Session, conversation: ChatConversation, **fields) -> ChatMessage:
    message = ChatMessage(conversation_id=conversation.id, **fields)
    db.add(message)
    conversation.messages.append(message)
    conversation.updated_at = datetime.now(UTC)
    db.commit()
    return message


HELD_BACK = (
    "Not done: a check found this probably isn't what the user asked for. Don't try it again "
    "unless they confirm. Tell them exactly what it would change and ask whether they want it."
)


def _held_back(
    db: Session, conversation: ChatConversation, call: ToolCall, scopes: Iterable[Scope]
) -> bool:
    """Whether a change the model asked for should be held back because the check model
    thinks the user didn't ask for it. Read-only actions, actions the chat isn't allowed to
    run anyway, and turns where checking is off or the check model fails all go ahead."""
    found = ACTIONS.get(call.name)
    if not found or found.scope == Scope.READ or found.scope not in set(scopes):
        return False
    if not checking_enabled(db):
        return False
    try:
        verdict = check_action(
            db, history(conversation), found.name, found.description, call.arguments
        )
    except ProviderError:
        return False  # already in the AI request logs
    return not verdict.allowed


def run_turn(
    db: Session,
    conversation: ChatConversation,
    user_text: str,
    *,
    scopes: Iterable[Scope] = (),
    today: date | None = None,
) -> Iterator[dict]:
    """Handle one user message, yielding progress events:
    {"event": "text", "delta"} as the model writes (text before a "tool" event is a preamble
    to that step), {"event": "tool", "name", "arguments"}, {"event": "tool_result", "name",
    "ok"}, then {"event": "message", "content"} with the full answer, or {"event": "error"}."""
    _add(db, conversation, role="user", content=user_text)
    try:
        provider, cfg, model = resolve_task(db, "chat")
    except ProviderError as e:
        yield {"event": "error", "detail": str(e)}
        return
    label = f"{provider.id}:{model}"
    tools = describe_actions(scopes)
    system = system_prompt(today or date.today(), [str(s) for s in scopes])

    for round_number in range(1, MAX_ROUNDS + 1):
        started = time.monotonic()
        reply = None
        # On the last round the model must answer with what it has: it's told so, and the
        # provider is asked not to let it call any more tools.
        last_round = round_number == MAX_ROUNDS
        prompt = system + LAST_ROUND if last_round else system
        try:
            for item in provider.stream_chat_turn(
                cfg, model, prompt, history(conversation), tools, answer_only=last_round
            ):
                if isinstance(item, ChatReply):
                    reply = item
                elif item:
                    yield {"event": "text", "delta": item}
            if reply is None:
                raise ProviderError(f"{provider.label} ended the response early.")
        except ProviderError as e:
            _log(db, label, None, None, started, str(e))
            db.commit()
            yield {"event": "error", "detail": str(e)}
            return
        _log(db, label, reply.request, reply.response, started, None)
        if last_round and reply.tool_calls:
            # A provider that can't switch tools off asked for more anyway. Keep any answer it
            # wrote, without the calls (or the raw reply holding them), so the history stays valid.
            if not reply.text.strip():
                break
            reply = replace(reply, tool_calls=[], raw=None)
        _add(
            db,
            conversation,
            role="assistant",
            content=reply.text,
            tool_calls=[
                {"id": c.id, "name": c.name, "arguments": c.arguments} for c in reply.tool_calls
            ],
            raw=reply.raw,
            raw_provider=provider.id,
        )
        if not reply.tool_calls:
            yield {"event": "message", "content": reply.text}
            return

        for call in reply.tool_calls:
            yield {"event": "tool", "name": call.name, "arguments": call.arguments}
            if _held_back(db, conversation, call, scopes):
                _add(
                    db,
                    conversation,
                    role="tool",
                    tool_call_id=call.id,
                    tool_name=call.name,
                    content=json.dumps({"error": HELD_BACK}),
                    is_error=True,
                )
                yield {"event": "tool_result", "name": call.name, "ok": False, "held": True}
                continue
            try:
                result = run_action(
                    db,
                    call.name,
                    call.arguments,
                    scopes=scopes,
                    source="chat",
                    conversation_id=conversation.id,
                )
                is_error = False
            except ActionError as e:
                result = {"error": str(e)}
                is_error = True
            message = _add(
                db,
                conversation,
                role="tool",
                tool_call_id=call.id,
                tool_name=call.name,
                content=json.dumps(result, default=str),
                is_error=is_error,
            )
            yield {"event": "tool_result", "name": call.name, "ok": not is_error}
            change = result.get("change")
            if change:
                db.get(ActionChange, change["id"]).chat_message_id = message.id
                db.commit()
                yield {"event": "change", "id": change["id"], "summary": change["summary"]}

    # Only reached when the last round still asked for tools and wrote no answer
    text = (
        "I couldn't finish working that out in a reasonable number of steps. "
        "Could you ask something more specific?"
    )
    _add(db, conversation, role="assistant", content=text)
    yield {"event": "message", "content": text}


def display_turns(db: Session, conversation: ChatConversation) -> list[dict]:
    """The conversation as the user saw it: their messages and the model's answers, each
    answer listing the tools used to reach it and any changes made along the way."""
    changes = {
        c.chat_message_id: c
        for c in db.execute(
            select(ActionChange).where(ActionChange.conversation_id == conversation.id)
        ).scalars()
    }
    turns: list[dict] = []
    tools: list[dict] = []
    made: list[dict] = []
    for m in conversation.messages:
        if m.role == "user":
            turns.append({"role": "user", "content": m.content or ""})
            tools, made = [], []
        elif m.role == "tool":
            tool = {"name": m.tool_name, "ok": not m.is_error}
            if m.is_error and m.content == json.dumps({"error": HELD_BACK}):
                tool["held"] = True
            tools.append(tool)
            change = changes.get(m.id)
            if change:
                made.append(
                    {
                        "id": change.id,
                        "summary": change.summary,
                        "undone": change.undone_at is not None,
                    }
                )
        elif not m.tool_calls:
            turns.append(
                {"role": "assistant", "content": m.content or "", "tools": tools, "changes": made}
            )
            tools, made = [], []
    return turns
