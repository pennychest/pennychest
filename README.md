<p align="center">
  <img src="logo.png" alt="PennyChest" width="200"><br>
  <img src="title.png" width="200">
</p>

<p align="center">
  <img src="https://img.shields.io/badge/python-3.11-3776AB?logo=python&logoColor=white" alt="Python 3.11">
  <img src="https://img.shields.io/badge/fastapi-009688?logo=fastapi&logoColor=white" alt="FastAPI">
  <img src="https://img.shields.io/badge/database-SQLite%20%7C%20PostgreSQL-003B57?logo=postgresql&logoColor=white" alt="SQLite | PostgreSQL">
  <a href="https://github.com/pennychest/pennychest/releases/latest"><img src="https://img.shields.io/github/v/release/pennychest/pennychest?logo=github&logoColor=white" alt="Release"></a>
  <img src="https://img.shields.io/badge/docker-ready-2496ED?logo=docker&logoColor=white" alt="Docker">
  <img src="https://img.shields.io/badge/license-MIT-green" alt="License MIT">
  <a href="https://pennychest.github.io"><img src="https://img.shields.io/badge/docs-pennychest.github.io-2E7D32" alt="Docs"></a>
</p>

A self-hosted personal finance app with real double-entry accounting. Import bank statements, auto-categorise transactions, and review your spending - all in a modern web UI that you own and run yourself.

**Documentation: [pennychest.github.io](https://pennychest.github.io)**

## Quick start

```bash
docker run -d -p 8000:8000 -v pennychest-data:/data ghcr.io/pennychest/pennychest:latest
```

Open **http://localhost:8000** and set a password straight away. To run it in the cloud with HTTPS, see [Deploying to Fly.io](https://pennychest.github.io/guide/deploy-fly/).

## Development

```bash
docker compose -f docker-compose.dev.yml up -d
```

See [Local setup](https://pennychest.github.io/guide/development/) for the services, seed data and working on plugins and these docs.

## Documentation

The full docs are at **[pennychest.github.io](https://pennychest.github.io)**:

- [Installation](https://pennychest.github.io/guide/installation/): the image, SQLite or PostgreSQL
- [Deploying to Fly.io](https://pennychest.github.io/guide/deploy-fly/): a public HTTPS instance in a few commands
- [Plugins](https://pennychest.github.io/guide/plugins/): importers and exporters, e.g. HSBC PDF statements and Beancount
- [Signing in](https://pennychest.github.io/guide/signing-in/): the first-run password and resetting it
- [Categorisation](https://pennychest.github.io/guide/categorisation/): rules, learning from your history and AI, and how well they work together
- [Card taps](https://pennychest.github.io/guide/card-taps/): record Apple Pay payments the moment you make them
- [AI integration](https://pennychest.github.io/guide/ai-integration/): providers, automatic AI tasks and chat
- [AI agents (MCP)](https://pennychest.github.io/guide/mcp/): connect Claude, ChatGPT and other assistants to your ledger
- [CI and deployment](https://pennychest.github.io/guide/deployment/): tests, images and releases

They're built from [`docs/`](docs) here and the plugin READMEs in [pennychest-plugins](https://github.com/pennychest/pennychest-plugins).

## Releases

Releases are on the [releases page](https://github.com/pennychest/pennychest/releases), and what changed in each one is in [CHANGELOG.md](CHANGELOG.md). Versions follow [semver](https://semver.org/); before `1.0.0` the API, database schema and plugin interface may still change between minor versions, and anything breaking is called out in the changelog.

## Licence

MIT. See [LICENSE](LICENSE).
