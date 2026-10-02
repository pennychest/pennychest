import json
from pathlib import Path

from sqlalchemy import select
from sqlalchemy.orm import Session

from pennychest.accounts.models import Account
from pennychest.core.lookup_models import MatchType
from pennychest.rules.models import Rule


def load_seed_accounts(db: Session) -> dict[str, int]:
    """Load UK seed accounts from JSON. Returns mapping of full_path -> id."""
    seed_file = Path(__file__).parent.parent / "seeds" / "uk_accounts.json"
    accounts_data = json.loads(seed_file.read_text())

    path_to_id: dict[str, int] = {}

    for acc_data in accounts_data:
        existing = db.execute(
            select(Account).where(Account.full_path == acc_data["full_path"])
        ).scalar_one_or_none()

        if existing:
            path_to_id[existing.full_path] = existing.id
            if existing.icon is None and acc_data.get("icon"):
                existing.icon = acc_data["icon"]
            continue

        parent_id = None
        parent_path = acc_data.get("parent_path")
        if parent_path and parent_path in path_to_id:
            parent_id = path_to_id[parent_path]

        account = Account(
            name=acc_data["name"],
            full_path=acc_data["full_path"],
            parent_id=parent_id,
            type=acc_data["type"],
            icon=acc_data.get("icon"),
        )
        db.add(account)
        db.flush()
        path_to_id[account.full_path] = account.id

    db.commit()
    return path_to_id


def has_accounts(db: Session) -> bool:
    result = db.execute(select(Account).limit(1))
    return result.scalar_one_or_none() is not None


def load_seed_rules(db: Session) -> int:
    """Load UK seed rules from JSON. Returns count of rules created."""
    seed_file = Path(__file__).parent.parent / "seeds" / "uk_rules.json"
    rules_data = json.loads(seed_file.read_text())

    # Build lookup maps
    match_types = db.execute(select(MatchType)).scalars().all()
    match_type_name_to_id = {m.name: m.id for m in match_types}

    accounts = db.execute(select(Account)).scalars().all()
    account_path_to_id = {a.full_path: a.id for a in accounts}

    created = 0
    for rule_data in rules_data:
        pattern = rule_data.get("pattern", "")
        match_type_name = rule_data.get("match_type", "substring")
        target_account_path = rule_data.get("target_account", "")
        priority = rule_data.get("priority", 0)

        match_type_id = match_type_name_to_id.get(match_type_name)
        target_account_id = account_path_to_id.get(target_account_path)

        if not match_type_id or not target_account_id or not pattern:
            continue

        # Skip if rule already exists
        existing = db.execute(
            select(Rule).where(
                Rule.pattern == pattern,
                Rule.match_type_id == match_type_id,
                Rule.target_account_id == target_account_id,
            )
        ).scalar_one_or_none()

        if existing:
            continue

        rule = Rule(
            pattern=pattern,
            match_type_id=match_type_id,
            target_account_id=target_account_id,
            priority=priority,
        )
        db.add(rule)
        created += 1

    db.commit()
    return created


def has_rules(db: Session) -> bool:
    result = db.execute(select(Rule).limit(1))
    return result.scalar_one_or_none() is not None
