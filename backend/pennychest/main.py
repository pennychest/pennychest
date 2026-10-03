from contextlib import asynccontextmanager
from pathlib import Path

from fastapi import Depends, FastAPI, HTTPException, Request
from fastapi.responses import FileResponse
from fastapi.staticfiles import StaticFiles
from sqlalchemy import select
from sqlalchemy.orm import Session

from pennychest.accounts.routes import router as accounts_router
from pennychest.actions.routes import router as actions_router
from pennychest.ai.routes import router as ai_router
from pennychest.auth.routes import require_session
from pennychest.auth.routes import router as auth_router
from pennychest.budgets.routes import router as budgets_router
from pennychest.core.database import SessionLocal, get_db
from pennychest.dashboard.routes import router as dashboard_router
from pennychest.core.seed import seed_lookup_tables
from pennychest.core.seed_accounts import has_accounts, has_rules, load_seed_accounts, load_seed_rules
from pennychest.imports.routes import router as imports_router
from pennychest.reports.routes import router as reports_router
from pennychest.rules.routes import router as rules_router
from pennychest.taps.routes import router as taps_router
from pennychest.transactions.routes import router as transactions_router

# Import all models so they're registered with Base.metadata
from pennychest.accounts.models import Base  # noqa: F401
from pennychest.settings.models import AppSetting  # noqa: F401
from pennychest.core.lookup_models import (  # noqa: F401
    BudgetPeriod,
    CategorisationSource,
    ImportSourceType,
    MatchType,
)
from pennychest.imports.models import ImportBatch, RawImportRow  # noqa: F401
from pennychest.rules.models import Rule  # noqa: F401
from pennychest.transactions.models import Posting, Transaction  # noqa: F401
from pennychest.ai.models import AIRequestLog  # noqa: F401
from pennychest.taps.models import CardTap, WalletCard  # noqa: F401
from pennychest.auth.models import AuthSession  # noqa: F401
from pennychest.budgets.models import Budget  # noqa: F401


@asynccontextmanager
async def lifespan(app: FastAPI):
    db = SessionLocal()
    try:
        seed_lookup_tables(db)
    finally:
        db.close()
    yield


app = FastAPI(
    title="PennyChest",
    description="Self-hosted personal finance with real double-entry accounting",
    version="0.2.0",
    lifespan=lifespan,
    # Every /api route needs a signed-in session unless auth.routes lists it as public.
    dependencies=[Depends(require_session)],
)



@app.middleware("http")
async def no_framing(request: Request, call_next):
    # Nothing in PennyChest is meant to be embedded, and the OAuth consent page must never be
    # (another site could frame it and trick a click on Allow).
    response = await call_next(request)
    response.headers["X-Frame-Options"] = "DENY"
    response.headers["Content-Security-Policy"] = "frame-ancestors 'none'"
    return response


app.include_router(auth_router)
app.include_router(accounts_router)
app.include_router(transactions_router)
app.include_router(rules_router)
app.include_router(reports_router)
app.include_router(imports_router)
app.include_router(ai_router)
app.include_router(taps_router)
app.include_router(budgets_router)
app.include_router(dashboard_router)
app.include_router(actions_router)


@app.get("/api/health")
def health():
    return {"status": "ok"}


@app.get("/api/setup/status")
def setup_status(db: Session = Depends(get_db)):
    return {
        "has_accounts": has_accounts(db),
        "has_rules": has_rules(db),
    }


@app.post("/api/setup/seed")
def load_seeds(db: Session = Depends(get_db)):
    if has_accounts(db):
        raise HTTPException(status_code=400, detail="Accounts already exist")
    path_to_id = load_seed_accounts(db)
    rules_created = load_seed_rules(db)
    return {"accounts_created": len(path_to_id), "rules_created": rules_created}


@app.post("/api/setup/sync-seed-icons")
def sync_seed_icons(db: Session = Depends(get_db)):
    """Backfill icons from seed data onto existing accounts that have no icon set."""
    load_seed_accounts(db)
    return {"ok": True}


@app.post("/api/setup/seed-rules")
def load_seed_rules_endpoint(db: Session = Depends(get_db)):
    """Load seed rules only (requires accounts to exist)."""
    if not has_accounts(db):
        raise HTTPException(status_code=400, detail="Load accounts first")
    if has_rules(db):
        raise HTTPException(status_code=400, detail="Rules already exist")
    rules_created = load_seed_rules(db)
    return {"rules_created": rules_created}


@app.get("/api/lookup/match-types")
def list_match_types(db: Session = Depends(get_db)):
    result = db.execute(select(MatchType)).scalars().all()
    return [{"id": m.id, "name": m.name} for m in result]


@app.get("/api/lookup/categorisation-sources")
def list_categorisation_sources(db: Session = Depends(get_db)):
    result = db.execute(select(CategorisationSource)).scalars().all()
    return [{"id": c.id, "name": c.name} for c in result]


# Serve static frontend files (in production, React build output is here)
static_dir = Path(__file__).parent.parent / "static"
if static_dir.exists():
    # Serve static assets (JS, CSS, icons, images, etc.) directly
    app.mount("/assets", StaticFiles(directory=str(static_dir / "assets")), name="assets")

    # Icon addresses that sites and connector directories look for by convention
    ICON_ALIASES = {"favicon.ico": "pwa-192x192.png", "apple-touch-icon.png": "pwa-192x192.png"}

    # Serve individual root-level static files (favicon, logo, etc.)
    @app.get("/{filename}", include_in_schema=False)
    def static_file(filename: str):
        if filename in ICON_ALIASES:
            return FileResponse(str(static_dir / ICON_ALIASES[filename]), media_type="image/png")
        path = static_dir / filename
        if path.is_file():
            return FileResponse(str(path))
        return FileResponse(str(static_dir / "index.html"))

    # SPA catch-all: any deeper path returns index.html so React Router works
    @app.get("/{full_path:path}", include_in_schema=False)
    def spa_fallback(full_path: str):
        return FileResponse(str(static_dir / "index.html"))
