from dataclasses import dataclass

from pennychest.ai.providers import Provider

MATCH_TYPES = ["substring", "prefix", "regex"]

_SYSTEM = (
    "You are a personal finance assistant. You suggest rules that automatically "
    "categorise bank transactions."
)


@dataclass
class TxnSample:
    description: str
    account_full_path: str


@dataclass
class ExistingRule:
    pattern: str
    match_type: str
    target_account_full_path: str


@dataclass
class RuleSuggestion:
    pattern: str
    match_type: str
    target_account_full_path: str
    priority: int
    description: str


def _schema(account_paths: list[str]) -> dict:
    return {
        "type": "object",
        "properties": {
            "rules": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "pattern": {"type": "string"},
                        "match_type": {"type": "string", "enum": MATCH_TYPES},
                        "target_account_full_path": {"type": "string", "enum": account_paths},
                        "priority": {
                            "type": "integer",
                            "description": "Between 1 and 100; higher is matched first.",
                        },
                        "description": {
                            "type": "string",
                            "description": "A brief factual note about what this pattern "
                            "matches, e.g. the merchant name or type. Do not mention "
                            "transaction counts.",
                        },
                    },
                    "required": [
                        "pattern",
                        "match_type",
                        "target_account_full_path",
                        "priority",
                        "description",
                    ],
                    "additionalProperties": False,
                },
            }
        },
        "required": ["rules"],
        "additionalProperties": False,
    }


def suggest_rules(
    provider: Provider,
    cfg: dict,
    model: str,
    transactions: list[TxnSample],
    account_paths: list[str],
    existing_rules: list[ExistingRule],
) -> tuple[list[RuleSuggestion], dict, dict]:
    """Returns (suggestions, request_payload, response_payload)."""
    paths = sorted(account_paths)
    account_list = "\n".join(f"- {p}" for p in paths)

    existing_rules_text = (
        "\n".join(
            f'- "{r.pattern}" ({r.match_type}) → {r.target_account_full_path}'
            for r in existing_rules
        )
        if existing_rules
        else "None"
    )

    if transactions:
        txn_section = "Recently categorised transactions:\n" + "\n".join(
            f'- "{t.description}" → {t.account_full_path}' for t in transactions
        )
        pattern_guideline = "- Only suggest rules for patterns that appear more than once, or are clearly distinctive (e.g. a known brand name)."
    else:
        txn_section = "No categorised transactions are available yet."
        pattern_guideline = "- Suggest useful starter rules based on common UK personal finance transaction patterns (well-known supermarkets, transport, utilities, streaming services, etc.) that match the available accounts above."

    prompt = f"""Suggest rules to auto-categorise bank transactions.

Existing rules (do not duplicate these):
{existing_rules_text}

Available accounts:
{account_list}

{txn_section}

Guidelines:
{pattern_guideline}
- Prefer "substring" match type. Use "prefix" only if the pattern is always at the start. Avoid "regex" unless necessary.
- Set priority between 1 and 100 (higher = matched first). Use 10 as the default.
- Do not suggest rules that duplicate or overlap with existing rules.
- Keep patterns short and general enough to match future variations.
- For description: write a brief factual note about what the pattern matches (e.g. "Major UK supermarket chain"). Do not mention transaction counts."""

    call = provider.complete_json(cfg, model, _SYSTEM, prompt, _schema(paths), "suggest_rules")
    known = set(paths)
    suggestions = []
    for r in call.data.get("rules", []):
        if not isinstance(r, dict):
            continue
        if not r.get("pattern") or r.get("match_type") not in MATCH_TYPES:
            continue
        if r.get("target_account_full_path") not in known:
            continue
        try:
            priority = min(max(int(r.get("priority") or 10), 1), 100)
        except (TypeError, ValueError):
            priority = 10
        suggestions.append(RuleSuggestion(
            pattern=r["pattern"],
            match_type=r["match_type"],
            target_account_full_path=r["target_account_full_path"],
            priority=priority,
            description=str(r.get("description") or ""),
        ))
    return suggestions, call.request, call.response
