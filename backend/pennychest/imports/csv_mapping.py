"""Working out which column of a CSV file holds what, for files the CSV importer doesn't
recognise by their column names.

Each field's column comes from, in order: a template the user saved for files with the same
headers, the column names, or the model the "csv_columns" task uses. A decision model answers a
choice question per field, with its confidence; a language model returns them all as JSON."""

import json
import time
from dataclasses import dataclass

from sqlalchemy import select
from sqlalchemy.orm import Session

from pennychest.ai.models import AIRequestLog
from pennychest.ai.providers import DecisionProvider, ProviderError, resolve_task
from pennychest.imports.csv_importer import FIELDS, _normalise_header, columns_by_name
from pennychest.imports.models import CsvTemplate

# Rows shown to the user and the model
SAMPLE_ROWS = 5
NONE = "none"

_QUESTIONS = {
    "date": "Which column of `csv` holds the date of each transaction?",
    "description": (
        "Which column of `csv` holds the description of each transaction: who was paid or who "
        "paid, the merchant or the reference?"
    ),
    "amount": (
        "Which column of `csv` holds each transaction's amount as one signed number, with money "
        "in and money out in the same column? Not a running balance."
    ),
    "money_out": (
        "Which column of `csv` holds only money paid out (debits), with money paid in in "
        "another column? Not a running balance."
    ),
    "money_in": (
        "Which column of `csv` holds only money paid in (credits), with money paid out in "
        "another column? Not a running balance."
    ),
}

_SYSTEM = "You read bank statement CSV files exported by banks and card providers."


@dataclass
class Suggestion:
    column: int | None
    source: str  # "template", "name" or "ai"
    confidence: float | None = None  # only decision models report one


def header_key(headers: list[str]) -> str:
    """Headers in one form, so a template matches files laid out the same way."""
    return json.dumps([_normalise_header(h) for h in headers])


def find_template(db: Session, headers: list[str]) -> CsvTemplate | None:
    """The most recently saved template for files with these headers."""
    return db.execute(
        select(CsvTemplate)
        .where(CsvTemplate.header_key == header_key(headers))
        .order_by(CsvTemplate.id.desc())
        .limit(1)
    ).scalar_one_or_none()


def by_name(headers: list[str]) -> dict[str, Suggestion]:
    return {field: Suggestion(column, "name") for field, column in columns_by_name(headers).items()}


def _with_decision_model(provider: DecisionProvider, cfg, model, headers, rows):
    options = {str(i): f"Column {i + 1}, headed {h!r}" for i, h in enumerate(headers)}
    options[NONE] = "No column holds this"
    state = {"csv": {"headers": headers, "rows": rows}}
    questions = {
        field: {"type": "choice", "instructions": question, "criteria": options}
        for field, question in _QUESTIONS.items()
    }
    call = provider.choose(cfg, model, state, questions)
    found = {}
    for field in _QUESTIONS:
        answer = call.data.get(field) or {}
        choice = answer.get("choice")
        if choice in options:
            column = None if choice == NONE else int(choice)
            found[field] = Suggestion(column, "ai", answer.get("confidence"))
    return found, call.request, call.response


def _with_llm(provider, cfg, model, headers, rows):
    listing = json.dumps({"headers": headers, "rows": rows}, indent=2)
    questions = "\n".join(
        f"- {field}: {question.replace('`csv`', 'the file')}"
        for field, question in _QUESTIONS.items()
    )
    prompt = f"""Here are the headers and first rows of a bank statement CSV file:

{listing}

For each field, give the index (counting from 0) of the column that holds it, or null if none
does.
{questions}"""
    column = {"type": ["integer", "null"]}
    schema = {
        "type": "object",
        "properties": {field: column for field in _QUESTIONS},
        "required": list(_QUESTIONS),
        "additionalProperties": False,
    }
    call = provider.complete_json(cfg, model, _SYSTEM, prompt, schema, "csv_columns")
    found = {}
    for field in _QUESTIONS:
        value = call.data.get(field)
        if value is None or (isinstance(value, int) and 0 <= value < len(headers)):
            found[field] = Suggestion(value, "ai")
    return found, call.request, call.response


def suggest_with_ai(
    db: Session, headers: list[str], rows: list[list[str]]
) -> dict[str, Suggestion]:
    """The model's column for each field it answered. Raises ProviderError if the task isn't
    set up or the model fails; the call is logged either way."""
    provider, cfg, model = resolve_task(db, "csv_columns")
    rows = rows[:SAMPLE_ROWS]
    started = time.monotonic()
    request = response = error = None
    try:
        ask = _with_decision_model if isinstance(provider, DecisionProvider) else _with_llm
        found, request, response = ask(provider, cfg, model, headers, rows)
    except ProviderError as e:
        error = str(e)
        raise
    finally:
        db.add(
            AIRequestLog(
                provider=f"{provider.id}:{model}",
                operation="csv_columns",
                request_data=request,
                response_data=response,
                duration_ms=int((time.monotonic() - started) * 1000),
                error=error,
            )
        )
        db.commit()
    found = {f: s for f, s in found.items() if s.column is None or s.column < len(headers)}
    # One signed amount column makes separate money in and out columns redundant
    if found.get("amount") and found["amount"].column is not None:
        for field in ("money_out", "money_in"):
            found.pop(field, None)
    return found


def suggest(
    db: Session, headers: list[str], rows: list[list[str]], use_ai: bool
) -> tuple[dict[str, Suggestion], CsvTemplate | None, str | None]:
    """Each field's column, the template it came from if any, and the model's error if it was
    asked and failed. Column names win over the model, which only fills the gaps."""
    template = find_template(db, headers)
    if template is not None:
        columns = {f: Suggestion(c, "template") for f, c in template.columns.items() if f in FIELDS}
        return columns, template, None
    columns = by_name(headers)
    if not use_ai:
        return columns, None, None
    try:
        from_ai = suggest_with_ai(db, headers, rows)
    except ProviderError as e:
        return columns, None, str(e)
    amounts = {"amount", "money_out", "money_in"}
    named_amount = bool(amounts & columns.keys())
    for field, suggestion in from_ai.items():
        if field in columns or suggestion.column is None:
            continue
        if field in amounts and named_amount:
            continue  # the names already say how amounts are laid out
        columns[field] = suggestion
    return columns, None, None
