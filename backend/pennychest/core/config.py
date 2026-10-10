from pydantic_settings import BaseSettings


class Settings(BaseSettings):
    # SQLite by default; set PENNYCHEST_DATABASE_URL to a postgresql+psycopg2:// URL to use Postgres.
    database_url: str = "sqlite:////data/pennychest.db"
    upload_dir: str = "/data/uploads"
    # Plugins installed from Settings live in a virtualenv here, next to the database.
    plugin_dir: str = "/data/plugins"
    # False to offer only the official plugin repository in Settings.
    allow_plugin_repositories: bool = True

    model_config = {"env_prefix": "PENNYCHEST_"}


settings = Settings()
