import json

from fastapi import APIRouter, Depends, HTTPException, UploadFile, File
from sqlalchemy import select
from sqlalchemy.orm import Session

from pennychest.accounts.models import Account
from pennychest.core.database import get_db
from pennychest.core.lookup_models import CategorisationSource, MatchType
from pennychest.rules.engine import (
    _get_match_type_map,
    find_all_matching_rules,
    find_conflicts,
    find_matching_rule,
    get_all_rules_sorted,
    matches_rule,
)
from pennychest.rules.models import Rule
from pennychest.rules.schemas import (
    RuleApplyPreview,
    RuleApplyPreviewItem,
    RuleConflict,
    RuleCreate,
    RulePreviewRequest,
    RulePreviewResponse,
    RuleResponse,
    RuleTestRequest,
    RuleTestResponse,
    RuleUpdate,
)
from pennychest.transactions.models import Posting, Transaction

router = APIRouter(prefix="/api/rules", tags=["rules"])


@router.get("", response_model=list[RuleResponse])
def list_rules(db: Session = Depends(get_db)):
    result = db.execute(select(Rule).order_by(Rule.priority.desc(), Rule.id))
    rules = result.scalars().all()

    match_type_map = _get_match_type_map(db)
    account_map = {
        a.id: a.full_path
        for a in db.execute(select(Account)).scalars().all()
    }

    return [
        RuleResponse(
            id=r.id,
            pattern=r.pattern,
            match_type_id=r.match_type_id,
            match_type_name=match_type_map.get(r.match_type_id),
            target_account_id=r.target_account_id,
            target_account_path=account_map.get(r.target_account_id),
            priority=r.priority,
            source=r.source,
            description=r.description,
        )
        for r in rules
    ]


@router.get("/export")
def export_rules(db: Session = Depends(get_db)):
    """Export all rules as JSON."""
    rules = db.execute(
        select(Rule).order_by(Rule.priority.desc(), Rule.id)
    ).scalars().all()

    match_type_map = _get_match_type_map(db)
    account_map = {
        a.id: a.full_path
        for a in db.execute(select(Account)).scalars().all()
    }

    exported = []
    for rule in rules:
        exported.append({
            "pattern": rule.pattern,
            "match_type": match_type_map.get(rule.match_type_id, "substring"),
            "target_account": account_map.get(rule.target_account_id, ""),
            "priority": rule.priority,
        })

    return exported


@router.post("/import")
def import_rules(rules_data: list[dict], db: Session = Depends(get_db)):
    """Import rules from JSON.

    Additive — adds new rules without removing existing ones.
    Skips rules whose target account doesn't exist.
    """
    match_types = db.execute(select(MatchType)).scalars().all()
    match_type_name_to_id = {m.name: m.id for m in match_types}

    accounts = db.execute(select(Account)).scalars().all()
    account_path_to_id = {a.full_path: a.id for a in accounts}

    imported = 0
    skipped = 0
    for rule_data in rules_data:
        pattern = rule_data.get("pattern", "")
        match_type_name = rule_data.get("match_type", "substring")
        target_account_path = rule_data.get("target_account", "")
        priority = rule_data.get("priority", 0)

        match_type_id = match_type_name_to_id.get(match_type_name)
        target_account_id = account_path_to_id.get(target_account_path)

        if not match_type_id or not target_account_id or not pattern:
            skipped += 1
            continue

        # Check if this exact rule already exists
        existing = db.execute(
            select(Rule).where(
                Rule.pattern == pattern,
                Rule.match_type_id == match_type_id,
                Rule.target_account_id == target_account_id,
            )
        ).scalar_one_or_none()

        if existing:
            skipped += 1
            continue

        rule = Rule(
            pattern=pattern,
            match_type_id=match_type_id,
            target_account_id=target_account_id,
            priority=priority,
            source="import",
        )
        db.add(rule)
        imported += 1

    db.commit()
    return {"imported": imported, "skipped": skipped}


@router.get("/{rule_id}", response_model=RuleResponse)
def get_rule(rule_id: int, db: Session = Depends(get_db)):
    rule = db.get(Rule, rule_id)
    if not rule:
        raise HTTPException(status_code=404, detail="Rule not found")

    match_type_map = _get_match_type_map(db)
    account = db.get(Account, rule.target_account_id)

    return RuleResponse(
        id=rule.id,
        pattern=rule.pattern,
        match_type_id=rule.match_type_id,
        match_type_name=match_type_map.get(rule.match_type_id),
        target_account_id=rule.target_account_id,
        target_account_path=account.full_path if account else None,
        priority=rule.priority,
        source=rule.source,
        description=rule.description,
    )


@router.post("", response_model=RuleResponse, status_code=201)
def create_rule(payload: RuleCreate, db: Session = Depends(get_db)):
    rule = Rule(**payload.model_dump())
    db.add(rule)
    db.commit()
    db.refresh(rule)

    match_type_map = _get_match_type_map(db)
    account = db.get(Account, rule.target_account_id)

    return RuleResponse(
        id=rule.id,
        pattern=rule.pattern,
        match_type_id=rule.match_type_id,
        match_type_name=match_type_map.get(rule.match_type_id),
        target_account_id=rule.target_account_id,
        target_account_path=account.full_path if account else None,
        priority=rule.priority,
        source=rule.source,
        description=rule.description,
    )


@router.put("/{rule_id}", response_model=RuleResponse)
def update_rule(rule_id: int, payload: RuleUpdate, db: Session = Depends(get_db)):
    rule = db.get(Rule, rule_id)
    if not rule:
        raise HTTPException(status_code=404, detail="Rule not found")

    update_data = payload.model_dump(exclude_unset=True)
    for key, value in update_data.items():
        setattr(rule, key, value)

    db.commit()
    db.refresh(rule)

    match_type_map = _get_match_type_map(db)
    account = db.get(Account, rule.target_account_id)

    return RuleResponse(
        id=rule.id,
        pattern=rule.pattern,
        match_type_id=rule.match_type_id,
        match_type_name=match_type_map.get(rule.match_type_id),
        target_account_id=rule.target_account_id,
        target_account_path=account.full_path if account else None,
        priority=rule.priority,
        source=rule.source,
        description=rule.description,
    )


@router.delete("/{rule_id}", status_code=204)
def delete_rule(rule_id: int, db: Session = Depends(get_db)):
    rule = db.get(Rule, rule_id)
    if not rule:
        raise HTTPException(status_code=404, detail="Rule not found")
    db.delete(rule)
    db.commit()


@router.post("/preview", response_model=RulePreviewResponse)
def preview_rule(payload: RulePreviewRequest, db: Session = Depends(get_db)):
    """Preview which transactions would match a rule pattern.

    Tests the pattern against existing transaction descriptions.
    """
    match_type_map = _get_match_type_map(db)
    match_type_name = match_type_map.get(payload.match_type_id, "substring")

    # Get recent transaction descriptions
    transactions = db.execute(
        select(Transaction.id, Transaction.description)
        .order_by(Transaction.date.desc())
        .limit(500)
    ).all()

    matching = []
    for txn_id, desc in transactions:
        if matches_rule(desc, payload.pattern, match_type_name):
            matching.append({"id": txn_id, "description": desc})

    # Find conflicting rules
    rules = get_all_rules_sorted(db)
    conflicts = find_conflicts(
        payload.pattern,
        match_type_name,
        rules,
        match_type_map,
        exclude_rule_id=payload.exclude_rule_id,
    )

    return RulePreviewResponse(
        matching_transactions=matching,
        match_count=len(matching),
        conflicts=[
            RuleConflict(
                rule_id=c.rule_id,
                pattern=c.pattern,
                match_type=c.match_type,
                priority=c.priority,
                target_account_id=c.target_account_id,
            )
            for c in conflicts
        ],
    )


@router.post("/test", response_model=RuleTestResponse)
def test_rule(payload: RuleTestRequest, db: Session = Depends(get_db)):
    """Test a rule pattern against a list of descriptions."""
    match_type_map = _get_match_type_map(db)
    match_type_name = match_type_map.get(payload.match_type_id, "substring")

    matching = [d for d in payload.descriptions if matches_rule(d, payload.pattern, match_type_name)]

    return RuleTestResponse(
        matching=matching,
        match_count=len(matching),
        total=len(payload.descriptions),
    )


@router.post("/apply-preview", response_model=RuleApplyPreview)
def apply_rules_preview(db: Session = Depends(get_db)):
    """Preview what re-applying all rules would change.

    Shows transactions that would be re-categorised, preserving manual categorisations.
    """
    rules = get_all_rules_sorted(db)
    match_type_map = _get_match_type_map(db)

    manual_source = db.execute(
        select(CategorisationSource).where(CategorisationSource.name == "manual")
    ).scalar_one()

    # Get all transactions with their postings
    transactions = db.execute(
        select(Transaction)
        .options()
        .order_by(Transaction.date.desc())
        .limit(1000)
    ).scalars().all()

    items = []
    for txn in transactions:
        match = find_matching_rule(txn.description, rules, match_type_map)
        if not match:
            continue

        # Find category posting
        postings = db.execute(
            select(Posting).where(Posting.transaction_id == txn.id)
        ).scalars().all()

        for posting in postings:
            if posting.categorised_by_id == manual_source.id:
                # Manual - would be skipped but show as warning
                if posting.account_id != match.target_account_id:
                    current_account = db.get(Account, posting.account_id)
                    target_account = db.get(Account, match.target_account_id)
                    items.append(
                        RuleApplyPreviewItem(
                            transaction_id=txn.id,
                            description=txn.description,
                            current_account_path=(
                                current_account.full_path if current_account else None
                            ),
                            new_account_path=(
                                target_account.full_path if target_account else None
                            ),
                            rule_id=match.rule_id,
                            rule_pattern=match.pattern,
                            is_manual=True,
                            would_change=True,
                        )
                    )
            elif posting.account_id != match.target_account_id:
                current_account = db.get(Account, posting.account_id)
                target_account = db.get(Account, match.target_account_id)
                items.append(
                    RuleApplyPreviewItem(
                        transaction_id=txn.id,
                        description=txn.description,
                        current_account_path=(
                            current_account.full_path if current_account else None
                        ),
                        new_account_path=(
                            target_account.full_path if target_account else None
                        ),
                        rule_id=match.rule_id,
                        rule_pattern=match.pattern,
                        is_manual=False,
                        would_change=True,
                    )
                )

    return RuleApplyPreview(
        items=items,
        would_change_count=sum(1 for i in items if i.would_change and not i.is_manual),
        manual_conflict_count=sum(1 for i in items if i.is_manual),
    )
