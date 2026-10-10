# Installation

## The image

Images are published to `ghcr.io/pennychest/pennychest` for amd64 and arm64:

| Tag | What it is |
|---|---|
| `latest` | the most recent [release](https://github.com/pennychest/pennychest/releases) |
| `0.2.0`, `0.2` | a release, pinned at the precision you want |
| `main` | built from every push to `main`: unreleased, opt-in |
| `sha-abc1234` | one specific commit, for rollbacks and bisecting |

To update, pull the image again and recreate the container; migrations run on startup. What changed in each release is in the [changelog](https://github.com/pennychest/pennychest/blob/main/CHANGELOG.md).

[Plugins](plugins.md) are installed from **Settings → Plugins**.

## SQLite (default)

PennyChest keeps everything in `/data`: the SQLite database at `/data/pennychest.db`, uploaded statements and installed plugins. A single container is all you need — just keep `/data` on a persistent volume:

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

## Settings

| Variable | Default | What it does |
|---|---|---|
| `PENNYCHEST_DATABASE_URL` | `sqlite:////data/pennychest.db` | The database |
| `PENNYCHEST_UPLOAD_DIR` | `/data/uploads` | Where imported statement files are kept |
| `PENNYCHEST_PLUGIN_DIR` | `/data/plugins` | Where plugins installed from Settings go |
| `PENNYCHEST_ALLOW_PLUGIN_REPOSITORIES` | `true` | `false` offers only the official plugin repository |

## Next step

Open the app straight away and [set a password](signing-in.md).
