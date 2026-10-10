<p align="center">
  <img src="assets/logo.png" alt="PennyChest" width="200"><br>
  <img src="assets/title.png" width="200">
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

<p align="center">
  <img src="assets/dashboard.png" alt="Dashboard" width="250">
  <img src="assets/categories.png" alt="Spending categories" width="250">
  <img src="assets/chat.png" alt="Chat" width="250">
</p>

<p align="center">
  <img src="assets/card-taps.gif" alt="Card taps" width="250"><br>
  An iPhone Wallet shortcut records each Apple Pay payment the moment you tap.
</p>

## Quick start

```bash
docker run -d -p 8000:8000 -v pennychest-data:/data ghcr.io/pennychest/pennychest:latest
```

Open **http://localhost:8000** and set a password straight away.

## Licence

MIT. See [LICENSE](LICENSE).
