"""Checking that a change the chat is about to make is one the user asked for, with the model
chosen for the "check_actions" task.

Chat changes data without asking first, so before each change a second model reads the
recent conversation and the proposed action and says how likely it is that the user asked for
it. Unlikely changes are held back and the chat is told to ask the user instead."""

import json
import time
from dataclasses import dataclass

from sqlalchemy.orm import Session

from pennychest.ai.config import get_task
from pennychest.ai.models import AIRequestLog
from pennychest.ai.providers import Provider, ProviderError, TypeSafeProvider, resolve_task

# Below this probability that the user asked for the action, it is held back.
MIN_PROBABILITY = 0.5
# How many of the latest user and assistant messages the check reads.
CONTEXT_MESSAGES = 6

_QUESTION = (
    "Did the user ask for `proposed_action`, or clearly agree to it, in `conversation`? "
    "The action, and the details in its arguments, should be what the user wanted."
)
_CRITERIA = {
    "true": "The user asked for this change, or agreed to it when the assistant offered it, "
    "and its details match what they said or what the assistant looked up for them",
    "false": "The user only asked a question, asked for something different, or the action "
    "changes more, or other things, than they asked for",
}

_SYSTEM = (
    "You check actions an AI assistant in a personal finance app is about to take on the "
    "user's behalf. You answer only whether the user asked for the action."
)


@dataclass
class Verdict:
    probability: float

    @property
    def allowed(self) -> bool:
        return self.probability >= MIN_PROBABILITY


def checking_enabled(db: Session) -> bool:
    provider_id, model = get_task(db, "check_actions")
    return bool(provider_id and model)


def _state(conversation: list[dict], name: str, description: str, arguments: dict) -> dict:
    return {
        "conversation": [
            {"role": m["role"], "content": m["content"]}
            for m in conversation
            if m["role"] in ("user", "assistant") and m.get("content")
        ][-CONTEXT_MESSAGES:],
        "proposed_action": {"name": name, "description": description, "arguments": arguments},
    }


def _with_jev(provider: TypeSafeProvider, cfg, model, state):
    questions = {"asked": {"type": "noul", "instructions": _QUESTION, "criteria": _CRITERIA}}
    call = provider.choose(cfg, model, state, questions)
    answer = call.data.get("asked") or {}
    probability = answer.get("noul")
    if not isinstance(probability, int | float):
        raise ProviderError(f"{provider.label} returned an unexpected response.")
    return float(probability), call.request, call.response


def _with_llm(provider: Provider, cfg, model, state):
    prompt = f"""{json.dumps(state, indent=2, default=str)}

{_QUESTION}

Yes: {_CRITERIA["true"]}.
No: {_CRITERIA["false"]}.

Give the probability, from 0 to 1, that the answer is yes."""
    schema = {
        "type": "object",
        "properties": {"probability": {"type": "number"}},
        "required": ["probability"],
        "additionalProperties": False,
    }
    call = provider.complete_json(cfg, model, _SYSTEM, prompt, schema, "check_action")
    probability = call.data.get("probability")
    if not isinstance(probability, int | float):
        raise ProviderError(f"{provider.label} returned an unexpected response.")
    return min(max(float(probability), 0.0), 1.0), call.request, call.response


def check_action(
    db: Session, conversation: list[dict], name: str, description: str, arguments: dict
) -> Verdict:
    """How likely it is that the user asked for this action, given the conversation so far (in
    the provider-neutral history form). Raises ProviderError if the model can't be reached or
    isn't set up; the call is logged either way."""
    provider, cfg, model = resolve_task(db, "check_actions")
    state = _state(conversation, name, description, arguments)
    started = time.monotonic()
    request = response = error = None
    try:
        if isinstance(provider, TypeSafeProvider):
            probability, request, response = _with_jev(provider, cfg, model, state)
        else:
            probability, request, response = _with_llm(provider, cfg, model, state)
    except ProviderError as e:
        error = str(e)
        raise
    finally:
        db.add(
            AIRequestLog(
                provider=f"{provider.id}:{model}",
                operation="check_action",
                request_data=request,
                response_data=response,
                duration_ms=int((time.monotonic() - started) * 1000),
                error=error,
            )
        )
        db.commit()
    return Verdict(probability)
