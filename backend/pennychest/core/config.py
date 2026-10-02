from pydantic_settings import BaseSettings


class Settings(BaseSettings):
    # SQLite by default; set PENNYCHEST_DATABASE_URL to a postgresql+psycopg2:// URL to use Postgres.
    database_url: str = "sqlite:////data/pennychest.db"
    upload_dir: str = "/data/uploads"

    model_config = {"env_prefix": "PENNYCHEST_"}


settings = Settings()
