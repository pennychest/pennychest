<p align="center">
  <img src="docs/assets/logo.png" alt="PennyChest" width="200"><br>
  <img src="docs/assets/title.png" width="200">
</p>

<p align="center">
  <img src="https://img.shields.io/badge/rust-1.97-DEA584?logo=rust&logoColor=white" alt="Rust 1.97">
  <img src="https://img.shields.io/badge/axum-000000?logo=rust&logoColor=white" alt="axum">
  <a href="https://github.com/pennychest/pennychest/releases/latest"><img src="https://img.shields.io/github/v/release/pennychest/pennychest?logo=github&logoColor=white" alt="Release"></a>
  <img src="https://img.shields.io/badge/docker-ready-2496ED?logo=docker&logoColor=white" alt="Docker">
  <img src="https://img.shields.io/badge/license-MIT-green" alt="License MIT">
  <a href="https://pennychest.github.io"><img src="https://img.shields.io/badge/docs-pennychest.github.io-2E7D32" alt="Docs"></a>
</p>

A self-hosted personal finance app with real double-entry accounting. Import bank statements, auto-categorise transactions, and review your spending - all in a modern web UI that you own and run yourself.

**Documentation: [pennychest.github.io](https://pennychest.github.io)**

<p align="center">
  <img src="docs/assets/dashboard.png" alt="Dashboard" width="250">
  <img src="docs/assets/categories.png" alt="Spending categories" width="250">
  <img src="docs/assets/chat.png" alt="Chat" width="250">
</p>

<p align="center">
  <img src="docs/assets/card-taps.gif" alt="Card taps" width="250"><br>
  An iPhone Wallet shortcut records each Apple Pay payment the moment you tap.
</p>

## Quick start

### In the cloud, in one click

[![Deploy to Render](https://render.com/images/deploy-to-render-button.svg)](https://render.com/deploy?repo=https://github.com/pennychest/pennychest)

Click **Deploy Blueprint**, then open the address Render gives you. It needs a paid Render plan for the disk your data is kept on. For Fly.io and other options, see the [Quick start guide](https://pennychest.github.io/guide/quick-start/).

### On your own computer

```bash
docker run -d -p 8000:8000 -v pennychest-data:/data ghcr.io/pennychest/pennychest:latest
```

Open **http://localhost:8000** and set a password straight away.

## Licence

MIT. See [LICENSE](LICENSE).
