import os
import tempfile

TEST_DATABASE_URL = os.environ.get(
    "PENNYCHEST_TEST_DATABASE_URL",
    f"sqlite:///{tempfile.mkdtemp()}/pennychest_test.db",
)
# The app's startup hook opens its own session, so it must see the test database too.
os.environ["PENNYCHEST_DATABASE_URL"] = TEST_DATABASE_URL
os.environ["PENNYCHEST_PLUGIN_DIR"] = f"{tempfile.mkdtemp()}/plugins"

import pytest  # noqa: E402
from fastapi.testclient import TestClient  # noqa: E402
from sqlalchemy.orm import sessionmaker  # noqa: E402

from pennychest.accounts.models import Base  # noqa: E402
from pennychest.auth.service import login_throttle  # noqa: E402
from pennychest.core.database import get_db, make_engine  # noqa: E402
from pennychest.core.seed import seed_lookup_tables  # noqa: E402
from pennychest.main import app  # noqa: E402

# Import all models
from pennychest.transactions.models import Transaction, Posting  # noqa: E402, F401
from pennychest.rules.models import Rule  # noqa: E402, F401
from pennychest.imports.models import ImportBatch, RawImportRow  # noqa: E402, F401
from pennychest.taps.models import CardTap, WalletCard  # noqa: E402, F401
from pennychest.auth.models import AuthSession  # noqa: E402, F401
from pennychest.budgets.models import Budget  # noqa: E402, F401
from pennychest.chat.models import ChatConversation, ChatMessage  # noqa: E402, F401
from pennychest.actions.models import ActionChange  # noqa: E402, F401
from pennychest.mcp.models import AccessToken, OAuthClient, OAuthCode  # noqa: E402, F401
from pennychest.core.lookup_models import (  # noqa: E402, F401
    BudgetPeriod,
    MatchType,
    CategorisationSource,
    ImportSourceType,
)


@pytest.fixture(autouse=True)
def fresh_learned_model():
    """Each test's database is rolled back, so a model trained in one mustn't leak into the next."""
    from pennychest.ai import learned

    learned.forget()
    yield
    learned.forget()


@pytest.fixture(scope="session")
def engine():
    eng = make_engine(TEST_DATABASE_URL)
    Base.metadata.create_all(eng)
    # Committed up front so the app's startup seeding finds the rows instead of
    # writing them and blocking on each test's open transaction.
    with sessionmaker(bind=eng)() as session:
        seed_lookup_tables(session)
    return eng


@pytest.fixture
def db_session(engine):
    connection = engine.connect()
    transaction = connection.begin()
    session = sessionmaker(bind=connection)()

    seed_lookup_tables(session)

    yield session

    session.close()
    transaction.rollback()
    connection.close()


@pytest.fixture(autouse=True)
def ai_tasks_off(db_session):
    """Every AI task starts off, so a test only runs the ones it sets up with use_model. (A new
    install has them on, running once a model is chosen.)"""
    from pennychest.ai.config import TASKS, set_task_enabled

    for task in TASKS:
        set_task_enabled(db_session, task, False)


TEST_PASSWORD = "correct horse battery"


@pytest.fixture
def anon_client(db_session):
    """A client that has not signed in."""
    def override_get_db():
        yield db_session

    app.dependency_overrides[get_db] = override_get_db
    login_throttle.reset()
    with TestClient(app) as c:
        yield c
    app.dependency_overrides.clear()


@pytest.fixture
def client(anon_client):
    """A client signed in with TEST_PASSWORD."""
    response = anon_client.post("/api/auth/setup", json={"password": TEST_PASSWORD})
    assert response.status_code == 204
    return anon_client


def use_model(db, task, provider_id, model, mode="automatic"):
    """Have `task` use this provider's model, running `mode` where it has the choice; with no
    provider, turn the task off."""
    from pennychest.ai.config import (
        MODE_TASKS,
        set_model,
        set_task_enabled,
        set_task_kind,
        set_task_mode,
    )
    from pennychest.ai.providers import PROVIDERS

    if provider_id is None:
        set_task_enabled(db, task, False)
        return
    kind = PROVIDERS[provider_id].kind
    set_model(db, kind, provider_id, model)
    set_task_enabled(db, task, True)
    set_task_kind(db, task, kind)
    if task in MODE_TASKS:
        set_task_mode(db, task, mode)
