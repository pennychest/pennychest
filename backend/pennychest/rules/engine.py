"""Rule engine: matching logic for auto-categorising transactions."""

import re
from dataclasses import dataclass

from sqlalchemy import select
from sqlalchemy.orm import Session

from pennychest.core.lookup_models import MatchType
from pennychest.rules.models import Rule


@dataclass
class RuleMatch:
    rule_id: int
    target_account_id: int
    pattern: str
    match_type: str
    priority: int


def _get_match_type_map(db: Session) -> dict[int, str]:
    """Build mapping of match_type_id -> name."""
    rows = db.execute(select(MatchType)).scalars().all()
    return {row.id: row.name for row in rows}


def matches_rule(description: str, pattern: str, match_type: str) -> bool:
    """Check if a description matches a rule pattern."""
    desc_upper = description.upper()
    pattern_upper = pattern.upper()

    if match_type == "substring":
        return pattern_upper in desc_upper
    elif match_type == "prefix":
        return desc_upper.startswith(pattern_upper)
    elif match_type == "regex":
        try:
            return bool(re.search(pattern, description, re.IGNORECASE))
        except re.error:
            return False
    return False


def find_matching_rule(
    description: str,
    rules: list[Rule],
    match_type_map: dict[int, str],
) -> RuleMatch | None:
    """Find the first matching rule for a description (highest priority first).

    Rules must be pre-sorted by priority DESC, id ASC.
    """
    for rule in rules:
        match_type = match_type_map.get(rule.match_type_id, "substring")
        if matches_rule(description, rule.pattern, match_type):
            return RuleMatch(
                rule_id=rule.id,
                target_account_id=rule.target_account_id,
                pattern=rule.pattern,
                match_type=match_type,
                priority=rule.priority,
            )
    return None


def get_all_rules_sorted(db: Session) -> list[Rule]:
    """Get all rules sorted by priority DESC, id ASC (first match wins)."""
    return list(
        db.execute(
            select(Rule).order_by(Rule.priority.desc(), Rule.id)
        ).scalars().all()
    )


def find_all_matching_rules(
    description: str,
    rules: list[Rule],
    match_type_map: dict[int, str],
) -> list[RuleMatch]:
    """Find ALL rules that match a description (for conflict detection)."""
    matches = []
    for rule in rules:
        match_type = match_type_map.get(rule.match_type_id, "substring")
        if matches_rule(description, rule.pattern, match_type):
            matches.append(
                RuleMatch(
                    rule_id=rule.id,
                    target_account_id=rule.target_account_id,
                    pattern=rule.pattern,
                    match_type=match_type,
                    priority=rule.priority,
                )
            )
    return matches


def preview_rule_on_descriptions(
    pattern: str,
    match_type: str,
    descriptions: list[str],
) -> list[str]:
    """Return descriptions that would match a given pattern."""
    return [d for d in descriptions if matches_rule(d, pattern, match_type)]


def find_conflicts(
    pattern: str,
    match_type: str,
    rules: list[Rule],
    match_type_map: dict[int, str],
    exclude_rule_id: int | None = None,
) -> list[RuleMatch]:
    """Find existing rules that would conflict with a new pattern.

    Two rules conflict if they can match the same description.
    We test each existing rule's pattern against the new pattern
    and vice versa to find overlaps.
    """
    conflicts = []
    for rule in rules:
        if exclude_rule_id and rule.id == exclude_rule_id:
            continue
        existing_type = match_type_map.get(rule.match_type_id, "substring")
        # Check if the new pattern matches the existing rule's pattern (as a description)
        # and vice versa — this is a heuristic for overlap detection
        if (
            matches_rule(rule.pattern, pattern, match_type)
            or matches_rule(pattern, rule.pattern, existing_type)
        ):
            conflicts.append(
                RuleMatch(
                    rule_id=rule.id,
                    target_account_id=rule.target_account_id,
                    pattern=rule.pattern,
                    match_type=existing_type,
                    priority=rule.priority,
                )
            )
    return conflicts
