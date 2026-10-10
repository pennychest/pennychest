"""Labelling recurring merchants (subscription, bill or other) and scoring how unusual each new
charge is, with the model the "insights" task uses. Runs after each import, or when asked from
the import's review page; the results are read by the recurring_payments and unusual_charges
actions.

A decision model answers a choice question per merchant and a score question per charge; LLMs
return the same answers as JSON. A failing model never breaks an import: the error is in the AI
request logs and the work is retried on the next import."""

import json
import time
from datetime import date, timedelta
from statistics import median

from sqlalchemy import select
from sqlalchemy.orm import Session

from pennychest.actions.common import money
from pennychest.actions.insights import find_recurring, merchant_key, spending_charges
from pennychest.ai.config import task_available
from pennychest.ai.models import AIRequestLog, MerchantLabel
from pennychest.ai.providers import DecisionProvider, Provider, ProviderError, resolve_task
from pennychest.core.database import SessionLocal
from pennychest.transactions.models import Transaction

BATCH_SIZE = 40
HISTORY_DAYS = 400
MERCHANT_HISTORY = 12
UNCATEGORISED_PATH = "Expenses:Uncategorised"

KINDS = {
    "subscription": "A subscription to a service the user signed up for, such as streaming, "
    "software, apps, a gym, a magazine or a membership",
    "bill": "A bill for a household essential or contract, such as rent, mortgage, council tax, "
    "energy, water, insurance, phone, broadband or a loan repayment",
    "other": "Not a commitment: ordinary spending that just happens regularly, such as "
    "groceries, coffee, lunches, fuel or transport fares",
}

# Score levels, low to high. Stored as level / (len - 1), so 0 is normal and 1 very unusual.
UNUSUAL_LEVELS = [
    "Normal: in line with what this merchant usually charges, or an everyday amount for the "
    "category",
    "A little different: somewhat more than usual, or a new merchant charging an ordinary amount",
    "Unusual: clearly more than this merchant usually charges, or a large one-off for the "
    "category",
    "Very unusual: far beyond anything seen before; could be a mistake, a duplicate or fraud",
]

_SYSTEM = "You review a person's bank transactions in a personal finance app."


def _log(db, provider, model, operation, started, request, response, error) -> None:
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


def _call(db: Session, operation: str, decision, llm):
    """Run `decision(provider, cfg, model)` or `llm(...)`, whichever suits the chosen provider,
    logging the call. Each returns (result, request, response)."""
    provider, cfg, model = resolve_task(db, "insights")
    started = time.monotonic()
    request = response = error = None
    try:
        ask = decision if isinstance(provider, DecisionProvider) else llm
        result, request, response = ask(provider, cfg, model)
    except ProviderError as e:
        error = str(e)
        raise
    finally:
        _log(db, provider, model, operation, started, request, response, error)
    return result


# Merchants


def _kinds_with_decision_model(merchants: list[dict]):
    def ask(provider: DecisionProvider, cfg, model):
        found, requests, responses = {}, [], []
        for start in range(0, len(merchants), BATCH_SIZE):
            batch = merchants[start:start + BATCH_SIZE]
            keys = [f"m{start + i}" for i in range(len(batch))]
            state = dict(zip(keys, batch))
            questions = {
                key: {
                    "type": "choice",
                    "instructions": f"What kind of regular payment is `{key}`?",
                    "criteria": KINDS,
                }
                for key in keys
            }
            call = provider.choose(cfg, model, state, questions)
            requests.append(call.request)
            responses.append(call.response)
            for key, merchant in zip(keys, batch):
                answer = call.data.get(key) or {}
                if answer.get("choice") in KINDS:
                    found[merchant["merchant"]] = (answer["choice"], answer.get("confidence"))
        return found, {"calls": requests}, {"calls": responses}

    return ask


def _kinds_with_llm(merchants: list[dict]):
    def ask(provider: Provider, cfg, model):
        kinds = "\n".join(f"- {k}: {text}" for k, text in KINDS.items())
        listing = json.dumps(list(enumerate(merchants)), indent=2)
        prompt = f"""These payments repeat regularly. Say what kind each one is.

Kinds:
{kinds}

Payments, as [index, details]:
{listing}"""
        schema = {
            "type": "object",
            "properties": {
                "labels": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "index": {"type": "integer"},
                            "kind": {"type": "string", "enum": list(KINDS)},
                        },
                        "required": ["index", "kind"],
                        "additionalProperties": False,
                    },
                }
            },
            "required": ["labels"],
            "additionalProperties": False,
        }
        call = provider.complete_json(cfg, model, _SYSTEM, prompt, schema, "label_merchants")
        found = {}
        for label in call.data.get("labels", []):
            index = label.get("index") if isinstance(label, dict) else None
            if isinstance(index, int) and 0 <= index < len(merchants) and label["kind"] in KINDS:
                found[merchants[index]["merchant"]] = (label["kind"], None)
        return found, call.request, call.response

    return ask


def label_merchants(db: Session, today: date) -> int:
    """Label recurring merchants that haven't been labelled yet. Returns how many were."""
    labelled = set(db.execute(select(MerchantLabel.merchant_key)).scalars())
    unlabelled = [r for r in find_recurring(db, today) if r["merchant_key"] not in labelled]
    if not unlabelled:
        return 0
    merchants = [
        {
            "merchant": r["merchant"],
            "category": r["category"],
            "cadence": r["cadence"],
            "amount": r["amount"],
        }
        for r in unlabelled
    ]
    found = _call(
        db, "label_merchants", _kinds_with_decision_model(merchants), _kinds_with_llm(merchants)
    )
    for r in unlabelled:
        if r["merchant"] in found:
            kind, confidence = found[r["merchant"]]
            db.add(MerchantLabel(merchant_key=r["merchant_key"], kind=kind, confidence=confidence))
    db.flush()
    return len(found)


# Unusual charges


def _context(charge: dict, history: list[dict]) -> dict:
    """What the model sees for one charge: it, the merchant's earlier charges and what's
    typical for the category over the past year."""
    before = [h for h in history if h["date"] < charge["date"] or (
        h["date"] == charge["date"] and h["id"] < charge["id"])]
    key = merchant_key(charge["description"])
    merchant = [h for h in before if merchant_key(h["description"]) == key][-MERCHANT_HISTORY:]
    context = {
        "charge": {
            "date": charge["date"].isoformat(),
            "description": charge["description"],
            "amount": money(charge["amount"]),
            "category": charge["category"],
        },
        "merchant_history": [
            {"date": h["date"].isoformat(), "amount": money(h["amount"])} for h in merchant
        ],
    }
    if charge["category"] != UNCATEGORISED_PATH:
        amounts = sorted(h["amount"] for h in before if h["category"] == charge["category"])
        if amounts:
            context["category_last_year"] = {
                "charges": len(amounts),
                "median": money(median(amounts)),
                "largest": money(amounts[-1]),
            }
    return context


def _scores_with_decision_model(contexts: dict[int, dict]):
    def ask(provider: DecisionProvider, cfg, model):
        ids = list(contexts)
        found, requests, responses = {}, [], []
        for start in range(0, len(ids), BATCH_SIZE):
            batch = ids[start:start + BATCH_SIZE]
            state = {f"c{i}": contexts[i] for i in batch}
            questions = {
                f"c{i}": {
                    "type": "score",
                    "instructions": (
                        f"How unusual is `c{i}.charge` compared with `c{i}.merchant_history` "
                        "and what is typical for its category?"
                    ),
                    "criteria": UNUSUAL_LEVELS,
                }
                for i in batch
            }
            call = provider.choose(cfg, model, state, questions)
            requests.append(call.request)
            responses.append(call.response)
            for i in batch:
                score = (call.data.get(f"c{i}") or {}).get("score")
                if isinstance(score, int | float):
                    found[i] = score
        return found, {"calls": requests}, {"calls": responses}

    return ask


def _scores_with_llm(contexts: dict[int, dict]):
    def ask(provider: Provider, cfg, model):
        levels = "\n".join(f"{n}: {text}" for n, text in enumerate(UNUSUAL_LEVELS))
        listing = json.dumps(contexts, indent=2)
        prompt = f"""Rate how unusual each new charge is, compared with the merchant's earlier \
charges and what is typical for its category.

Levels:
{levels}

Charges, keyed by id:
{listing}"""
        schema = {
            "type": "object",
            "properties": {
                "scores": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {"type": "integer"},
                            "level": {"type": "integer", "enum": list(range(len(UNUSUAL_LEVELS)))},
                        },
                        "required": ["id", "level"],
                        "additionalProperties": False,
                    },
                }
            },
            "required": ["scores"],
            "additionalProperties": False,
        }
        call = provider.complete_json(cfg, model, _SYSTEM, prompt, schema, "score_charges")
        found = {}
        for item in call.data.get("scores", []):
            if not isinstance(item, dict):
                continue
            i, level = item.get("id"), item.get("level")
            if i in contexts and isinstance(level, int | float):
                found[i] = level
        return found, call.request, call.response

    return ask


def score_charges(db: Session, transaction_ids: list[int]) -> int:
    """Score how unusual each of these transactions' spending is. Returns how many were."""
    txns = db.execute(
        select(Transaction.id, Transaction.date).where(Transaction.id.in_(transaction_ids))
    ).all()
    if not txns:
        return 0
    first, last = min(d for _, d in txns), max(d for _, d in txns)
    history = sorted(
        spending_charges(db, first - timedelta(days=HISTORY_DAYS), last).values(),
        key=lambda c: (c["date"], c["id"]),
    )
    wanted = set(transaction_ids)
    contexts = {
        c["id"]: _context(c, [h for h in history if h["date"] >= c["date"] - timedelta(
            days=HISTORY_DAYS)])
        for c in history
        if c["id"] in wanted
    }
    if not contexts:
        return 0
    scores = _call(
        db, "score_charges", _scores_with_decision_model(contexts), _scores_with_llm(contexts)
    )
    top = len(UNUSUAL_LEVELS) - 1
    for txn_id, score in scores.items():
        db.get(Transaction, txn_id).unusual_score = round(min(max(score / top, 0.0), 1.0), 3)
    db.flush()
    return len(scores)


def run_after_import(db: Session, batch_id: int) -> str | None:
    """Score the import's charges and label recurring merchants that are new as of its latest
    charge, if insights are on and have a model. Returns the model's error, if it failed."""
    if not task_available(db, "insights"):
        return None
    rows = db.execute(
        select(Transaction.id, Transaction.date).where(Transaction.import_batch_id == batch_id)
    ).all()
    if not rows:
        return None
    try:
        score_charges(db, [i for i, _ in rows])
        label_merchants(db, max(d for _, d in rows))
    except ProviderError as e:
        db.commit()  # keep the logs
        return str(e)
    db.commit()
    return None


def run_in_background(batch_id: int) -> None:
    """Run after the import response has been sent, in its own session."""
    with SessionLocal() as db:
        run_after_import(db, batch_id)
