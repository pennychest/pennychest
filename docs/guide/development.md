# Local setup

## 1. Start the services

```bash
docker compose -f docker-compose.dev.yml up -d
```

This starts:

| Service    | Purpose                                                  |
|------------|----------------------------------------------------------|
| `db`       | PostgreSQL                                               |
| `api`      | FastAPI backend (runs migrations automatically on startup) |
| `frontend` | React dev server with hot reload                         |
| `ollama`   | Self-hosted LLM (optional, for [AI categorisation](ai-integration.md)) |

The app is available at **http://localhost:8000**.

## 2. Seed initial data (first run only)

On first load the app prompts you to load seed data — a UK-oriented chart of accounts and default categorisation rules. Follow the on-screen setup flow.

## Plugins

To work on importers or exporters from your own checkout of pennychest-plugins, see [Developing a plugin](plugins.md#developing-a-plugin).

## Working on these docs

```bash
pip install -r docs/requirements.txt
mkdocs serve
```

The site is served at **http://localhost:8000** by default; pass `-a localhost:8001` if the app is already using that port.

The published site, [pennychest.github.io](https://pennychest.github.io), is built by the [pennychest.github.io repository](https://github.com/pennychest/pennychest.github.io) from these docs and the plugin READMEs in pennychest-plugins. Pushing changes under `docs/` or to `mkdocs.yml` on `main` rebuilds it.
