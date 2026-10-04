# Installation

## SQLite (default)

PennyChest stores everything in SQLite at `/data/pennychest.db` by default, so a single container is all you need — just keep `/data` on a persistent volume:

```bash
docker run -d -p 8000:8000 -v pennychest-data:/data <image>
```

!!! warning
    Only run one container against a SQLite database.

## PostgreSQL

To use PostgreSQL instead, set `PENNYCHEST_DATABASE_URL`, e.g.:

```
postgresql+psycopg2://user:pass@host:5432/pennychest
```

Migrations run automatically on startup either way.

## Next step

Open the app straight away and [set a password](signing-in.md).
