# PennyChest

A self-hosted personal finance app with real double-entry accounting. Import bank statements, auto-categorise transactions, and review your spending — all in a modern web UI that you own and run yourself.

## Tech stack

- **Backend:** Python / FastAPI / SQLAlchemy / SQLite or PostgreSQL
- **Frontend:** React / TanStack Router / TanStack Query / shadcn/ui
- **Deployment:** a single Docker container, or Docker Compose

## Where next

- [Installation](guide/installation.md) — run PennyChest with SQLite or PostgreSQL
- [Deploying to Fly.io](guide/deploy-fly.md) — a public HTTPS instance in a few commands
- [Plugins](guide/plugins.md) — importers and exporters, e.g. HSBC PDFs and Beancount
- [Signing in](guide/signing-in.md) — first-run password and resetting it
- [Categorisation](guide/categorisation.md) — rules, learning from your history and AI, and how well they work together
- [Card taps](guide/card-taps.md) — record Apple Pay payments the moment you make them
- [AI integration](guide/ai-integration.md) — providers, automatic AI tasks and chat
- [AI agents (MCP)](guide/mcp.md) — connect Claude, ChatGPT and other assistants to your ledger
- [Local setup](guide/development.md) — the development stack
- [CI and deployment](guide/deployment.md) — tests, builds and Fly.io

## Licence

MIT.
