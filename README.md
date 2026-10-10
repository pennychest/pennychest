<p align="center">
  <img src="assets/logo.png" alt="PennyChest" width="200"><br>
  <img src="assets/title.png" alt="" width="200">
</p>

<p align="center">
  <a href="https://github.com/pennychest/pennychest/releases/latest"><img src="https://img.shields.io/github/v/release/pennychest/pennychest?logo=github&logoColor=white" alt="Release"></a>
  <img src="https://img.shields.io/badge/docker-ready-2496ED?logo=docker&logoColor=white" alt="Docker">
  <img src="https://img.shields.io/badge/license-MIT-green" alt="License MIT">
  <a href="https://pennychest.github.io"><img src="https://img.shields.io/badge/docs-pennychest.github.io-2E7D32" alt="Docs"></a>
</p>

PennyChest is a personal finance app you run yourself. Import your bank statements and it sorts every transaction into a category, shows where your money goes, and answers questions about it. Your data stays on your own server.

<p align="center">
  <img src="assets/dashboard.png" alt="The dashboard, with spending by category this year" width="250">
  <img src="assets/categories.png" alt="Spending categories" width="250">
  <img src="assets/chat.png" alt="Asking the chat about saving money" width="250">
</p>

## What it does

- **Categorises for you, carefully.** Your rules come first, then a model that learns from how you've categorised before, then AI. Each only answers when it's sure, so on real spending it got 98.8% right while doing 83% automatically. The rest comes to you to review.
- **Records card payments as you make them.** An iPhone shortcut sends each Apple Pay payment to PennyChest the moment you tap, already categorised, and matches it to your statement when it arrives.
- **Shows where your money goes.** A dashboard you can customise, budgets, and spending by category, month or merchant.
- **Answers questions.** Ask "what are my subscriptions?" or "how much did I spend eating out last month?" in the chat, or connect Claude, ChatGPT or another AI assistant to your ledger.
- **Real double-entry accounting.** Every penny is accounted for, and transfers between your accounts aren't counted as spending.
- **Plugins.** Install importers for more banks and exporters to other formats from Settings, or make your own.

AI is optional: without it, PennyChest still categorises with your rules and your own history, and nothing leaves your server.

<p align="center">
  <img src="assets/card-taps.gif" alt="Paying with Apple Pay, and the payment appearing in PennyChest" width="250">
</p>

## Get started

Put it in the cloud in one click, no command line needed:

[![Deploy to Render](https://render.com/images/deploy-to-render-button.svg)](https://render.com/deploy?repo=https://github.com/pennychest/pennychest)

Or run it on your own computer with Docker:

```bash
docker run -d -p 8000:8000 -v pennychest-data:/data ghcr.io/pennychest/pennychest:latest
```

and open http://localhost:8000. Either way, set a password straight away.

## Documentation

Everything else, from other ways to host it to AI providers and plugins, is at **[pennychest.github.io](https://pennychest.github.io)**.

## Licence

MIT. See [LICENSE](LICENSE).
