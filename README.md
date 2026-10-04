# PennyChest

Self-hosted personal finance with real double-entry accounting. Import bank statements, auto-categorise transactions with rules, and review your spending — all in a modern web UI that you own and run yourself.

## Tech stack

- **Backend:** Python / FastAPI / SQLAlchemy / SQLite or PostgreSQL
- **Frontend:** React / TanStack Router / TanStack Query / shadcn/ui
- **Deployment:** a single Docker container, or Docker Compose

## Database

PennyChest stores everything in SQLite at `/data/pennychest.db` by default, so a single container is all you need — just keep `/data` on a persistent volume:

```bash
docker run -d -p 8000:8000 -v pennychest-data:/data <image>
```

To use PostgreSQL instead, set `PENNYCHEST_DATABASE_URL`, e.g. `postgresql+psycopg2://user:pass@host:5432/pennychest`. Migrations run automatically on startup either way. Only run one container against a SQLite database.

## Plugins

Importers and exporters are plugins: Python packages that register themselves as entry points (`pennychest.importers`, `pennychest.exporters`). The core includes the CSV importer. Optional ones, such as the HSBC PDF statement importer and the Beancount exporter, live in [pennychest-plugins](https://github.com/pennychest/pennychest-plugins).

To include plugins in the image, pass them as pip requirements when building, pinned to a tag or commit:

```bash
docker build \
  --build-arg PENNYCHEST_PLUGINS="git+https://github.com/pennychest/pennychest-plugins@hsbc-v0.1.0#subdirectory=hsbc" \
  -t pennychest .
```

Separate several plugins with spaces. Outside Docker, `pip install` the same requirement into PennyChest's environment and restart it.

For plugin development, clone pennychest-plugins next to this repository. The dev Compose setup mounts it at `/plugins`, so `docker compose -f docker-compose.dev.yml exec api pip install -e /plugins/<name>` (then restart `api`) runs a plugin from your working copy.

## Signing in

The first time you open PennyChest it asks you to create a password; after that every device has to sign in. Open it yourself straight after deploying, since until a password is set anyone who reaches the page can choose one. Serve it over HTTPS when it's reachable from the internet.

Forgotten your password? Run this in the container, then open the app to choose a new one:

```bash
docker exec <container> python -m pennychest.auth.reset_password
```

## Deployment

Pull requests and pushes to `main` run the backend tests, every plugin's tests from pennychest-plugins against this core, and a frontend build.

This repository doesn't deploy anything itself. To run PennyChest on a host such as Fly.io, build the image with the plugins you want (see [Plugins](#plugins)) and keep `/data` on a persistent volume. Run a single machine when using SQLite, since the database lives on one volume.

## Development setup

### 1. Start the services

```bash
docker compose -f docker-compose.dev.yml up -d
```

This starts:
- `db` — PostgreSQL
- `api` — FastAPI backend (runs migrations automatically on startup)
- `frontend` — React dev server with hot reload
- `ollama` — self-hosted LLM (optional, for AI categorisation)

The app is available at **http://localhost:8000**.

### 2. Seed initial data (first run only)

On first load the app will prompt you to load seed data (a UK-oriented chart of accounts and default categorisation rules). Follow the on-screen setup flow.

### 3. AI integration (optional)

PennyChest can use an LLM to suggest categories for uncategorised transactions and propose new rules.

#### Option A — Ollama (self-hosted, free)

Pull the recommended model after starting the services:

```bash
docker exec pc-ollama-1 ollama pull qwen3:4b
```

Then in the app go to **Settings → AI Integration**, switch the provider to **Ollama**, and set:
- **URL:** `http://ollama:11434`
- **Model:** `qwen3:4b`

#### Option B — Claude (Anthropic API)

Get an API key from [console.anthropic.com](https://console.anthropic.com). In the app go to **Settings → AI Integration**, keep the provider as **Claude**, and paste your key.

> The Anthropic API is billed per token and is separate from a claude.ai subscription.

## Licence

MIT. See [LICENSE](LICENSE).
