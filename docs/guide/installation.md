# Installation

## The image

Images are published to `ghcr.io/pennychest/pennychest` for amd64 and arm64:

| Tag | What it is |
|---|---|
| `latest` | the most recent [release](https://github.com/pennychest/pennychest/releases) |
| `0.1.0`, `0.1` | a release, pinned at the precision you want |
| `main` | built from every push to `main`: unreleased, opt-in |
| `sha-abc1234` | one specific commit, for rollbacks and bisecting |

To update, pull the image again and recreate the container; migrations run on startup. What changed in each release is in the [changelog](https://github.com/pennychest/pennychest/blob/main/CHANGELOG.md).

The published image doesn't include any [plugins](plugins.md). To add some, [build the image yourself](plugins.md#building-them-into-the-image).

## SQLite (default)

PennyChest stores everything in SQLite at `/data/pennychest.db` by default, so a single container is all you need — just keep `/data` on a persistent volume:

```bash
docker run -d -p 8000:8000 -v pennychest-data:/data ghcr.io/pennychest/pennychest:latest
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
