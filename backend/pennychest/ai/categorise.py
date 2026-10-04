from dataclasses import dataclass

from pennychest.ai.providers import Provider, ProviderError, TypeSafeProvider

# Jev reports calibrated confidence; below this, leave the transaction uncategorised
# rather than guess. Behind the learned model, 0.9 got 98.8% of a test set right while still
# categorising 83% of it automatically, against 95.0% right on 90% at 0.5.
JEV_MIN_CONFIDENCE = 0.9
JEV_BATCH_SIZE = 40
JEV_MAX_OPTIONS = 255

_SYSTEM = (
    "You are a personal finance assistant. You assign bank transactions to the most "
    "appropriate account from a fixed list."
)


@dataclass
class TxnInput:
    id: int
    description: str


@dataclass
class Categorisation:
    transaction_id: int
    account_full_path: str
    confidence: float | None = None  # only decision models such as Jev report one


def _schema(account_paths: list[str]) -> dict:
    return {
        "type": "object",
        "properties": {
            "categorisations": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "transaction_id": {"type": "integer"},
                        "account_full_path": {"type": "string", "enum": account_paths},
                    },
                    "required": ["transaction_id", "account_full_path"],
                    "additionalProperties": False,
                },
            }
        },
        "required": ["categorisations"],
        "additionalProperties": False,
    }


def _with_llm(provider, cfg, model, transactions, account_paths):
    account_list = "\n".join(f"- {p}" for p in account_paths)
    txn_list = "\n".join(f"- id={t.id}: {t.description}" for t in transactions)
    prompt = f"""Assign each bank transaction to the most appropriate account.

Available accounts:
{account_list}

Transactions to categorise:
{txn_list}

Return one entry per transaction id, using only accounts from the list above."""

    call = provider.complete_json(
        cfg, model, _SYSTEM, prompt, _schema(account_paths), "categorise_transactions"
    )
    return [
        Categorisation(c["transaction_id"], c["account_full_path"])
        for c in call.data.get("categorisations", [])
        if isinstance(c, dict) and c.get("transaction_id") and c.get("account_full_path")
    ], call.request, call.response


def _with_jev(provider: TypeSafeProvider, cfg, model, transactions, account_paths):
    if len(account_paths) > JEV_MAX_OPTIONS:
        raise ProviderError(
            f"Jev can choose between at most {JEV_MAX_OPTIONS} accounts, "
            f"but you have {len(account_paths)}."
        )
    criteria = {path: None for path in account_paths}
    results, requests, responses = [], [], []
    for start in range(0, len(transactions), JEV_BATCH_SIZE):
        batch = transactions[start:start + JEV_BATCH_SIZE]
        # One shared state and one Choice question per transaction, answered in a single call.
        state = {f"t{t.id}": t.description for t in batch}
        questions = {
            f"t{t.id}": {
                "type": "choice",
                "instructions": (
                    f"Which account should the bank transaction `t{t.id}` be recorded "
                    "against? Accounts are colon-separated paths from general to specific."
                ),
                "criteria": criteria,
            }
            for t in batch
        }
        call = provider.choose(cfg, model, state, questions)
        requests.append(call.request)
        responses.append(call.response)
        for t in batch:
            answer = call.data.get(f"t{t.id}") or {}
            if (
                answer.get("choice") in criteria
                and (answer.get("confidence") or 0) >= JEV_MIN_CONFIDENCE
            ):
                results.append(Categorisation(t.id, answer["choice"], answer.get("confidence")))
    return results, {"calls": requests}, {"calls": responses}


def categorise_transactions(
    provider: Provider,
    cfg: dict,
    model: str,
    transactions: list[TxnInput],
    account_paths: list[str],
) -> tuple[list[Categorisation], dict, dict]:
    """Returns (categorisations, request_payload, response_payload). Only answers that name
    one of the given transactions and accounts are returned."""
    paths = sorted(account_paths)
    if isinstance(provider, TypeSafeProvider):
        results, request, response = _with_jev(provider, cfg, model, transactions, paths)
    else:
        results, request, response = _with_llm(provider, cfg, model, transactions, paths)
    ids = {t.id for t in transactions}
    known = set(paths)
    valid = [c for c in results if c.transaction_id in ids and c.account_full_path in known]
    return valid, request, response
