from logging.config import fileConfig

from alembic import context
from sqlalchemy import engine_from_config, pool

from pennychest.accounts.models import Base
from pennychest.core.config import settings
# Import all models to register them
from pennychest.transactions.models import Transaction, Posting  # noqa: F401
from pennychest.rules.models import Rule  # noqa: F401
from pennychest.imports.models import ImportBatch, RawImportRow  # noqa: F401
from pennychest.budgets.models import Budget  # noqa: F401
from pennychest.core.lookup_models import (  # noqa: F401
    BudgetPeriod,
    MatchType,
    CategorisationSource,
    ImportSourceType,
)

config = context.config
if config.config_file_name is not None:
    fileConfig(config.config_file_name)

# Use the app's database URL (SQLite by default, or PENNYCHEST_DATABASE_URL).
# "%" is escaped because Alembic's config applies ConfigParser interpolation.
config.set_main_option("sqlalchemy.url", settings.database_url.replace("%", "%%"))

target_metadata = Base.metadata


def run_migrations_offline() -> None:
    url = config.get_main_option("sqlalchemy.url")
    context.configure(
        url=url,
        target_metadata=target_metadata,
        literal_binds=True,
        dialect_opts={"paramstyle": "named"},
    )
    with context.begin_transaction():
        context.run_migrations()


def run_migrations_online() -> None:
    connectable = engine_from_config(
        config.get_section(config.config_ini_section, {}),
        prefix="sqlalchemy.",
        poolclass=pool.NullPool,
    )
    with connectable.connect() as connection:
        context.configure(connection=connection, target_metadata=target_metadata)
        with context.begin_transaction():
            context.run_migrations()


if context.is_offline_mode():
    run_migrations_offline()
else:
    run_migrations_online()
