import time
from dataclasses import dataclass

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel
from sqlalchemy import select
from sqlalchemy.orm import Session, joinedload

from pennychest.accounts.models import Account
from pennychest.actions.models import ActionChange
from pennychest.ai import insights, learned
from pennychest.ai.categorise import Categorisation, TxnInput, categorise_transactions
from pennychest.ai.config import (
    AUTOMATIC,
    CHAT_SCOPES,
    KIND_LABELS,
    MODE_TASKS,
    ON_DEMAND,
    TASK_KINDS,
    TASKS,
    cache_models,
    cached_models,
    get_chat_scopes,
    get_model,
    get_provider_problem,
    get_task,
    provider_value,
    set_chat_scopes,
    set_model,
    set_provider_problem,
    set_provider_values,
    set_task_enabled,
    set_task_kind,
    set_task_mode,
    task_enabled,
    task_kind,
    task_mode,
)
from pennychest.ai.models import AIRequestLog
from pennychest.ai.providers import (
    PROVIDERS,
    CredentialsError,
    ProviderError,
    missing_fields,
    provider_config,
    resolve_task,
)
from pennychest.ai.rules import ExistingRule, RuleSuggestion, TxnSample, suggest_rules
from pennychest.core.database import get_db
from pennychest.core.lookup_models import CategorisationSource
from pennychest.imports.models import ImportBatch
from pennychest.rules.engine import _get_match_type_map
from pennychest.rules.models import Rule
from pennychest.transactions.models import Posting, Transaction

router = APIRouter(prefix="/api/ai", tags=["ai"])

_UNCATEGORISED_PATH = "Expenses:Uncategorised"


def _save_ai_log(
    db: Session,
    provider: str,
    operation: str,
    request_data: dict,
    response_data: dict | None,
    duration_ms: int,
    error: str | None,
) -> None:
    log = AIRequestLog(
        provider=provider,
        operation=operation,
        request_data=request_data,
        response_data=response_data,
        duration_ms=duration_ms,
        error=error,
    )
    db.add(log)
    db.commit()


def _get_ai_source(db: Session) -> CategorisationSource:
    return db.execute(
        select(CategorisationSource).where(CategorisationSource.name == "ai")
    ).scalar_one()


def _get_uncategorised_account_id(db: Session) -> int | None:
    account = db.execute(
        select(Account).where(Account.full_path == _UNCATEGORISED_PATH)
    ).scalar_one_or_none()
    return account.id if account else None


def _all_account_paths(db: Session) -> list[str]:
    accounts = db.execute(select(Account)).scalars().all()
    return [
        a.full_path
        for a in accounts
        if a.full_path != _UNCATEGORISED_PATH
    ]


def _task_label(db: Session, task: str) -> str:
    """How a task's provider and model are recorded in the AI logs."""
    provider_id, model = get_task(db, task)
    return f"{provider_id}:{model}" if provider_id else "not configured"


def _run_categorise(
    txn_inputs: list[TxnInput], account_paths: list[str], db: Session
) -> tuple[list[Categorisation], str, dict, dict]:
    """Returns (categorisations, provider, request_payload, response_payload)."""
    provider, cfg, model = resolve_task(db, "categorise")
    result, req, resp = categorise_transactions(provider, cfg, model, txn_inputs, account_paths)
    return result, _task_label(db, "categorise"), req, resp


def _run_suggest_rules(
    txn_samples: list[TxnSample],
    account_paths: list[str],
    existing_rules: list[ExistingRule],
    db: Session,
) -> tuple[list[RuleSuggestion], str, dict, dict]:
    """Returns (suggestions, provider, request_payload, response_payload)."""
    provider, cfg, model = resolve_task(db, "rules")
    result, req, resp = suggest_rules(
        provider, cfg, model, txn_samples, account_paths, existing_rules
    )
    return result, _task_label(db, "rules"), req, resp


def _apply_categorisations(categorisations, account_path_to_id, ai_source_id, uncategorised_id, db):
    """Move each still-uncategorised posting to its AI category. Returns the postings as they
    were before, for undo."""
    before = []
    for c in categorisations:
        target_id = account_path_to_id.get(c.account_full_path)
        if not target_id:
            continue

        posting = db.execute(
            select(Posting)
            .join(CategorisationSource, Posting.categorised_by_id == CategorisationSource.id)
            .where(
                Posting.transaction_id == c.transaction_id,
                Posting.account_id == uncategorised_id,
                CategorisationSource.name == "import_default",
            )
        ).scalar_one_or_none()

        if not posting:
            continue

        before.append(
            {
                "id": posting.id,
                "account_id": posting.account_id,
                "categorised_by_id": posting.categorised_by_id,
                "rule_id": posting.rule_id,
            }
        )
        posting.account_id = target_id
        posting.categorised_by_id = ai_source_id
        posting.rule_id = None

    db.commit()
    return before


@dataclass
class CategoriseOutcome:
    learned: int = 0  # categorised from the user's own history
    ai: int = 0  # categorised by the AI provider
    error: str | None = None  # why the AI couldn't help, if it couldn't

    @property
    def total(self) -> int:
        return self.learned + self.ai


def categorise_uncategorised(
    db: Session, *, batch: ImportBatch | None = None, source: str = "api", use_ai: bool = True
) -> CategoriseOutcome:
    """Categorise transactions an import left uncategorised (in one batch, or everywhere):
    first from the user's own history, when that's confident, then the rest with the AI
    provider. Records what changed as one undoable change. The AI's errors are reported in
    the outcome (and kept in the AI logs) rather than raised, so what was learned still
    stands."""
    outcome = CategoriseOutcome()
    uncategorised_id = _get_uncategorised_account_id(db)
    if not uncategorised_id:
        return outcome

    query = (
        select(Posting)
        .join(Transaction, Posting.transaction_id == Transaction.id)
        .join(CategorisationSource, Posting.categorised_by_id == CategorisationSource.id)
        .where(
            Posting.account_id == uncategorised_id,
            CategorisationSource.name == "import_default",
        )
        .options(joinedload(Posting.transaction))
    )
    if batch is not None:
        query = query.where(Transaction.import_batch_id == batch.id)
    postings = db.execute(query).scalars().all()
    if not postings:
        return outcome

    before: list[dict] = []
    learned_source = db.execute(
        select(CategorisationSource).where(CategorisationSource.name == "learned")
    ).scalar_one()
    remaining = []
    for posting, suggestion in zip(
        postings, learned.suggest(db, [p.transaction.description for p in postings])
    ):
        if suggestion is None:
            remaining.append(posting)
            continue
        before.append(
            {
                "id": posting.id,
                "account_id": posting.account_id,
                "categorised_by_id": posting.categorised_by_id,
                "rule_id": posting.rule_id,
            }
        )
        posting.account_id = suggestion[0]
        posting.categorised_by_id = learned_source.id
        posting.rule_id = None
    outcome.learned = len(before)
    db.commit()

    if remaining and use_ai:
        txn_inputs = [
            TxnInput(id=p.transaction_id, description=p.transaction.description)
            for p in remaining
        ]
        account_paths = _all_account_paths(db)
        account_path_to_id = {
            a.full_path: a.id for a in db.execute(select(Account)).scalars().all()
        }
        start = time.monotonic()
        try:
            categorisations, provider, req_payload, resp_payload = _run_categorise(
                txn_inputs, account_paths, db
            )
        except ValueError as e:
            elapsed = int((time.monotonic() - start) * 1000)
            _save_ai_log(
                db, _task_label(db, "categorise"), "categorise", None, None, elapsed, str(e)
            )
            outcome.error = str(e)
        else:
            elapsed = int((time.monotonic() - start) * 1000)
            _save_ai_log(db, provider, "categorise", req_payload, resp_payload, elapsed, None)
            by_ai = _apply_categorisations(
                categorisations, account_path_to_id, _get_ai_source(db).id, uncategorised_id, db
            )
            outcome.ai = len(by_ai)
            before += by_ai

    if before:
        count = f"{len(before)} transaction{'s' if len(before) != 1 else ''}"
        where = f" from {batch.file_name}" if batch is not None and batch.file_name else ""
        how = f" ({outcome.learned} from your history)" if outcome.learned else ""
        db.add(
            ActionChange(
                action="recategorise_transactions",
                arguments={"import_batch_id": batch.id if batch is not None else None},
                summary=f"Categorised {count}{where}{how}",
                # Same shape as recategorise_transactions, so it undoes the same way
                undo={"postings": before},
                source=source,
            )
        )
        db.commit()
    return outcome


def _categorise_response(outcome: CategoriseOutcome) -> dict:
    if outcome.error and not outcome.total:
        raise HTTPException(status_code=400, detail=outcome.error)
    response = {"updated": outcome.total}
    if outcome.learned:
        response["learned"] = outcome.learned
    if outcome.error:
        response["error"] = outcome.error
    return response


@router.post("/categorise/batch/{batch_id}")
def ai_categorise_batch(batch_id: int, db: Session = Depends(get_db)):
    """Categorise all uncategorised transactions in a specific import batch."""
    batch = db.get(ImportBatch, batch_id)
    if not batch:
        raise HTTPException(status_code=404, detail="Import batch not found")
    return _categorise_response(categorise_uncategorised(db, batch=batch))


@router.post("/insights/batch/{batch_id}")
def ai_insights_batch(batch_id: int, db: Session = Depends(get_db)):
    """Label the recurring payments and score the charges in an import, for when insights run
    on demand rather than straight after each import."""
    if not db.get(ImportBatch, batch_id):
        raise HTTPException(status_code=404, detail="Import batch not found")
    try:
        resolve_task(db, "insights")
    except ProviderError as e:
        raise HTTPException(status_code=400, detail=str(e))
    error = insights.run_after_import(db, batch_id)
    if error:
        raise HTTPException(status_code=400, detail=error)
    return {"ok": True}


@router.post("/categorise/all")
def ai_categorise_all(db: Session = Depends(get_db)):
    """Categorise all uncategorised transactions across all batches."""
    return _categorise_response(categorise_uncategorised(db))


@router.post("/suggest-rules")
def ai_suggest_rules(db: Session = Depends(get_db)):
    """Suggest categorisation rules based on manually and AI-categorised transactions."""
    # Get recent manually/AI categorised transactions (up to 200)
    ai_and_manual = db.execute(
        select(CategorisationSource).where(
            CategorisationSource.name.in_(["manual", "ai"])
        )
    ).scalars().all()
    source_ids = [s.id for s in ai_and_manual]

    postings = db.execute(
        select(Posting)
        .where(Posting.categorised_by_id.in_(source_ids))
        .options(
            joinedload(Posting.transaction),
            joinedload(Posting.account),
        )
        .order_by(Posting.id.desc())
        .limit(200)
    ).scalars().all()

    txn_samples = [
        TxnSample(
            description=p.transaction.description,
            account_full_path=p.account.full_path,
        )
        for p in postings
        if p.transaction and p.account
    ]

    # Build existing rules with their match type names
    rules = db.execute(select(Rule)).scalars().all()
    match_type_map = _get_match_type_map(db)
    account_id_to_path = {
        a.id: a.full_path
        for a in db.execute(select(Account)).scalars().all()
    }
    existing_rules = [
        ExistingRule(
            pattern=r.pattern,
            match_type=match_type_map.get(r.match_type_id, "substring"),
            target_account_full_path=account_id_to_path.get(r.target_account_id, ""),
        )
        for r in rules
    ]

    account_paths = _all_account_paths(db)
    start = time.monotonic()
    try:
        suggestions, provider, req_payload, resp_payload = _run_suggest_rules(txn_samples, account_paths, existing_rules, db)
    except ValueError as e:
        _save_ai_log(db, _task_label(db, "rules"), "suggest_rules", None, None, int((time.monotonic() - start) * 1000), str(e))
        raise HTTPException(status_code=400, detail=str(e))

    _save_ai_log(db, provider, "suggest_rules", req_payload, resp_payload, int((time.monotonic() - start) * 1000), None)

    # Map account paths back to IDs for the frontend
    account_path_to_id = {v: k for k, v in account_id_to_path.items()}
    match_type_name_to_id = {v: k for k, v in match_type_map.items()}

    return {
        "suggestions": [
            {
                "pattern": s.pattern,
                "match_type": s.match_type,
                "match_type_id": match_type_name_to_id.get(s.match_type),
                "target_account_full_path": s.target_account_full_path,
                "target_account_id": account_path_to_id.get(s.target_account_full_path),
                "priority": s.priority,
                "description": s.description,
            }
            for s in suggestions
        ]
    }


@router.get("/logs")
def list_ai_logs(
    limit: int = 100,
    offset: int = 0,
    db: Session = Depends(get_db),
):
    logs = (
        db.execute(
            select(AIRequestLog)
            .order_by(AIRequestLog.created_at.desc())
            .limit(limit)
            .offset(offset)
        )
        .scalars()
        .all()
    )
    from sqlalchemy import func as sqlfunc
    total = db.execute(select(sqlfunc.count()).select_from(AIRequestLog)).scalar_one()
    return {
        "total": total,
        "logs": [
            {
                "id": log.id,
                "created_at": log.created_at.isoformat(),
                "provider": log.provider,
                "operation": log.operation,
                "request_data": log.request_data,
                "response_data": log.response_data,
                "duration_ms": log.duration_ms,
                "error": log.error,
            }
            for log in logs
        ],
    }


class ProviderValues(BaseModel):
    # Omit a field to leave it unchanged; send null or "" to clear it.
    values: dict[str, str | None]


class ModelChoice(BaseModel):
    provider: str | None
    model: str | None = None


class TaskSettings(BaseModel):
    # Omit any to leave it unchanged
    enabled: bool | None = None
    kind: str | None = None
    mode: str | None = None


def _provider_or_404(provider_id: str):
    provider = PROVIDERS.get(provider_id)
    if not provider:
        raise HTTPException(status_code=404, detail="Unknown AI provider")
    return provider


@router.get("/config")
def get_ai_config(db: Session = Depends(get_db)):
    providers = []
    for provider in PROVIDERS.values():
        cfg = provider_config(db, provider)
        models, updated_at = cached_models(db, provider.id)
        providers.append({
            "id": provider.id,
            "label": provider.label,
            "kind": provider.kind,
            "configured": not missing_fields(provider, cfg),
            # Set when the saved credentials were rejected the last time they were tried
            "problem": get_provider_problem(db, provider.id),
            "fields": [
                {
                    "key": f.key,
                    "label": f.label,
                    "secret": f.secret,
                    "required": f.required,
                    "placeholder": f.placeholder,
                    "is_set": bool(provider_value(db, provider.id, f.key)),
                    # Secrets never leave the server.
                    "value": None if f.secret else cfg.get(f.key),
                }
                for f in provider.fields
            ],
            "models": models,
            "models_updated_at": updated_at,
        })

    models = {}
    for kind in KIND_LABELS:
        provider_id, model = get_model(db, kind)
        provider = PROVIDERS.get(provider_id or "")
        problem = None
        if provider is not None:
            missing = missing_fields(provider, provider_config(db, provider))
            problem = get_provider_problem(db, provider.id) or (
                f"Add {provider.label}'s {', '.join(missing)} below." if missing else None
            )
        models[kind] = {
            "provider": provider_id,
            "model": model,
            "ready": bool(provider and model) and problem is None,
            "problem": problem,
        }

    tasks = {}
    for task, label in TASKS.items():
        provider_id, model = get_task(db, task)
        try:
            resolve_task(db, task)
            problem = None
        except ProviderError as e:
            problem = str(e)
        tasks[task] = {
            "label": label,
            # The kinds of model it can use, and the one it does
            "kinds": list(TASK_KINDS[task]),
            "kind": task_kind(db, task),
            "enabled": task_enabled(db, task),
            # "automatic" or "on_demand", or None if it doesn't have the choice
            "mode": task_mode(db, task),
            "provider": provider_id,
            "model": model,
            "ready": problem is None,
            "problem": problem,
        }
    tasks["chat"]["scopes"] = get_chat_scopes(db)
    return {"providers": providers, "models": models, "tasks": tasks}


@router.put("/providers/{provider_id}", status_code=204)
def save_provider(provider_id: str, body: ProviderValues, db: Session = Depends(get_db)):
    provider = _provider_or_404(provider_id)
    known = {f.key for f in provider.fields}
    unknown = set(body.values) - known
    if unknown:
        raise HTTPException(status_code=400, detail=f"Unknown fields: {', '.join(sorted(unknown))}")
    set_provider_values(db, provider.id, body.values)


@router.post("/providers/{provider_id}/models")
def refresh_models(provider_id: str, db: Session = Depends(get_db)):
    """Fetch the provider's current model list and remember it."""
    provider = _provider_or_404(provider_id)
    cfg = provider_config(db, provider)
    missing = missing_fields(provider, cfg)
    if missing:
        raise HTTPException(
            status_code=400,
            detail=f"Add {provider.label}'s {', '.join(missing)} first.",
        )
    try:
        models = provider.list_models(cfg)
    except ProviderError as e:
        # Only a rejected key is remembered; a busy or unreachable provider may work next time
        if isinstance(e, CredentialsError):
            set_provider_problem(db, provider.id, str(e))
        raise HTTPException(status_code=400, detail=str(e))
    set_provider_problem(db, provider.id, None)
    updated_at = cache_models(db, provider.id, models)
    return {"models": models, "models_updated_at": updated_at}


@router.put("/models/{kind}", status_code=204)
def choose_model(kind: str, body: ModelChoice, db: Session = Depends(get_db)):
    """Choose the language model or decision model the tasks share, or clear it."""
    if kind not in KIND_LABELS:
        raise HTTPException(status_code=404, detail="Unknown kind of model")
    if body.provider is not None:
        provider = _provider_or_404(body.provider)
        if provider.kind != kind:
            raise HTTPException(
                status_code=400, detail=f"{provider.label} isn't a {KIND_LABELS[kind]}."
            )
        if not (body.model or "").strip():
            raise HTTPException(status_code=400, detail="Choose a model")
    set_model(db, kind, body.provider, (body.model or "").strip() or None)


@router.put("/tasks/{task}", status_code=204)
def update_task(task: str, body: TaskSettings, db: Session = Depends(get_db)):
    """Turn a task on or off, choose which kind of model it uses, and whether it runs by
    itself or only when asked."""
    if task not in TASKS:
        raise HTTPException(status_code=404, detail="Unknown AI task")
    if body.kind is not None and body.kind not in TASK_KINDS[task]:
        raise HTTPException(
            status_code=400,
            detail=f"{TASKS[task]} can't use a {KIND_LABELS.get(body.kind, body.kind)}.",
        )
    modes = (AUTOMATIC, ON_DEMAND)
    if body.mode is not None and (task not in MODE_TASKS or body.mode not in modes):
        raise HTTPException(status_code=400, detail=f"{TASKS[task]} can't run {body.mode}.")
    if body.enabled is not None:
        set_task_enabled(db, task, body.enabled)
    if body.kind is not None:
        set_task_kind(db, task, body.kind)
    if body.mode is not None:
        set_task_mode(db, task, body.mode)


class ChatScopes(BaseModel):
    scopes: list[str]


@router.put("/chat/scopes", status_code=204)
def choose_chat_scopes(body: ChatScopes, db: Session = Depends(get_db)):
    """What the chat may change: "organise" (categories, rules and budgets) and/or
    "transactions". Looking things up is always allowed."""
    unknown = set(body.scopes) - set(CHAT_SCOPES)
    if unknown:
        raise HTTPException(status_code=400, detail=f"Unknown scopes: {', '.join(sorted(unknown))}")
    set_chat_scopes(db, body.scopes)


@router.get("/learned")
def learned_status(db: Session = Depends(get_db)):
    """What the categoriser that learns from the user's history knows, and how it does."""
    return learned.status(db)


class EnabledBody(BaseModel):
    enabled: bool


@router.put("/learned", status_code=204)
def set_learned(body: EnabledBody, db: Session = Depends(get_db)):
    learned.set_learned_enabled(db, body.enabled)
