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
</p>

A self-hosted personal finance app with real double-entry accounting. Import bank statements, auto-categorise transactions, and review your spending - all in a modern web UI that you own and run yourself.

## Quick start

```bash
docker run -d -p 8000:8000 -v pennychest-data:/data ghcr.io/pennychest/pennychest:latest
```

Open **http://localhost:8000** and set a password straight away.

## Development

```bash
docker compose -f docker-compose.dev.yml up -d
```

## Documentation

PostgreSQL, plugins (HSBC statements, Beancount export), sign-in and password resets, AI categorisation, and CI/deployment are covered in the docs:

```bash
uvx --with "mkdocs-material>=9.5" "mkdocs>=1.6,<2" serve -a localhost:8001
```

## Releases

Releases are on the [releases page](https://github.com/pennychest/pennychest/releases), and what changed in each one is in [CHANGELOG.md](CHANGELOG.md). Versions follow [semver](https://semver.org/); before `1.0.0` the API, database schema and plugin interface may still change between minor versions, and anything breaking is called out in the changelog.

## Licence

MIT. See [LICENSE](LICENSE).
