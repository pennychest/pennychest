# Installation

## The image

Images are published to `ghcr.io/pennychest/pennychest` for amd64 and arm64:

| Tag | What it is |
|---|---|
| `latest` | the most recent [release](https://github.com/pennychest/pennychest/releases) |
| `0.2.0`, `0.2` | a release, pinned at the precision you want |
| `main` | built from every push to `main`: unreleased, opt-in |
| `sha-abc1234` | one specific commit, for rollbacks and bisecting |

The image holds a single program with the app built into it, and the certificates it needs to reach AI providers; there's no shell or anything else inside. To update, pull the image again and recreate the container: the database is brought up to date as PennyChest starts. What changed in each release is in the [changelog](https://github.com/pennychest/pennychest/blob/main/CHANGELOG.md). Take a [backup](../backups.md) first if you'd like to be able to go back.

## Running it

PennyChest keeps everything in `/data`: the SQLite database at `/data/pennychest.db`, uploaded statements and installed [plugins](plugins.md). A single container is all you need. Keep `/data` on a persistent volume:

```bash
docker run -d -p 8000:8000 -v pennychest-data:/data ghcr.io/pennychest/pennychest:latest
```

Or with Docker Compose:

```yaml
services:
  pennychest:
    image: ghcr.io/pennychest/pennychest:latest
    ports:
      - "8000:8000"
    volumes:
      - pennychest-data:/data
    restart: unless-stopped

volumes:
  pennychest-data:
```

The image reports its health to Docker, so `docker ps` shows whether it's answering.

!!! warning
    Only run one container against a database.

## Settings

| Variable | Default | What it does |
|---|---|---|
| `PENNYCHEST_DATABASE_PATH` | `/data/pennychest.db` | The SQLite database |
| `PENNYCHEST_UPLOAD_DIR` | `/data/uploads` | Where imported statement files are kept |
| `PENNYCHEST_PLUGIN_DIR` | `/data/plugins` | Where plugins installed from Settings go |
| `PENNYCHEST_ALLOW_PLUGIN_REPOSITORIES` | `true` | `false` offers only the official plugin repository |
| `PENNYCHEST_HOST` | `0.0.0.0` | The address to listen on |
| `PENNYCHEST_PORT` | `8000` | The port to listen on |
| `RUST_LOG` | `info` | How much to log, e.g. `debug` |

PennyChest only uses SQLite.

## Next step

Open the app straight away and [set a password](signing-in.md).
