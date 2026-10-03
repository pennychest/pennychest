"""Asking the model chosen for the "matching" task whether two records are the same thing,
where exact matching falls short: a card tap and a statement line, two imported transactions,
or the two sides of a transfer.

Jev answers one Noul (yes/no) question per pair; LLMs return a probability per pair. Callers
act only on pairs at or above MIN_PROBABILITY."""

import json
import time

from sqlalchemy.orm import Session

from pennychest.ai.config import get_task
from pennychest.ai.models import AIRequestLog
from pennychest.ai.providers import Provider, ProviderError, TypeSafeProvider, resolve_task

MIN_PROBABILITY = 0.8
BATCH_SIZE = 40

_SYSTEM = (
    "You compare financial records from a personal finance app and judge whether two of them "
    "are related in the way described."
)


def matching_enabled(db: Session) -> bool:
    provider_id, model = get_task(db, "matching")
    return bool(provider_id and model)


def _with_jev(provider: TypeSafeProvider, cfg, model, question, pairs):
    """`question` is worded with `{a}` and `{b}` standing for the two records."""
    probabilities, requests, responses = [], [], []
    for start in range(0, len(pairs), BATCH_SIZE):
        batch = pairs[start:start + BATCH_SIZE]
        keys = [f"p{start + i}" for i in range(len(batch))]
        state = {key: {"a": a, "b": b} for key, (a, b) in zip(keys, batch)}
        questions = {
            key: {
                "type": "noul",
                "instructions": question.format(a=f"`{key}.a`", b=f"`{key}.b`"),
            }
            for key in keys
        }
        call = provider.choose(cfg, model, state, questions)
        requests.append(call.request)
        responses.append(call.response)
        for key in keys:
            value = (call.data.get(key) or {}).get("noul")
            probabilities.append(float(value) if isinstance(value, int | float) else 0.0)
    return probabilities, {"calls": requests}, {"calls": responses}


def _with_llm(provider: Provider, cfg, model, question, pairs):
    listing = json.dumps(
        [{"pair": i, "a": a, "b": b} for i, (a, b) in enumerate(pairs)], indent=2, default=str
    )
    prompt = f"""For each pair below, answer: {question.format(a="`a`", b="`b`")}

{listing}

Give each pair's probability, from 0 to 1, that the answer is yes."""
    schema = {
        "type": "object",
        "properties": {
            "answers": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "pair": {"type": "integer"},
                        "probability": {"type": "number"},
                    },
                    "required": ["pair", "probability"],
                    "additionalProperties": False,
                },
            }
        },
        "required": ["answers"],
        "additionalProperties": False,
    }
    call = provider.complete_json(cfg, model, _SYSTEM, prompt, schema, "match_pairs")
    probabilities = [0.0] * len(pairs)
    for answer in call.data.get("answers", []):
        if not isinstance(answer, dict):
            continue
        index, value = answer.get("pair"), answer.get("probability")
        if isinstance(index, int) and 0 <= index < len(pairs) and isinstance(value, int | float):
            probabilities[index] = min(max(float(value), 0.0), 1.0)
    return probabilities, call.request, call.response


def ask_pairs(
    db: Session, operation: str, question: str, pairs: list[tuple[dict, dict]]
) -> list[float]:
    """The probability that the answer to `question` is yes for each (a, b) pair, in order.
    `question` refers to the records as `{a}` and `{b}`. Raises ProviderError if the model
    can't be reached or isn't set up; the call is logged under `operation` either way."""
    if not pairs:
        return []
    provider, cfg, model = resolve_task(db, "matching")
    started = time.monotonic()
    request = response = error = None
    try:
        if isinstance(provider, TypeSafeProvider):
            result, request, response = _with_jev(provider, cfg, model, question, pairs)
        else:
            result, request, response = _with_llm(provider, cfg, model, question, pairs)
    except ProviderError as e:
        error = str(e)
        raise
    finally:
        db.add(
            AIRequestLog(
                provider=f"{provider.id}:{model}",
                operation=operation,
                request_data=request,
                response_data=response,
                duration_ms=int((time.monotonic() - started) * 1000),
                error=error,
            )
        )
        db.flush()
    return result
