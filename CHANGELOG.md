# Changelog

All notable changes to this project are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Until 1.0.0 the API, database schema and plugin interface may still change between minor
versions; anything breaking will be called out here.

## v0.1.0 (2026-10-04)

The first release.

### Ledger

- Real double-entry accounting: every transaction is a set of balanced postings, with
  transfers between accounts linked on both sides.
- A UK chart of accounts to start from, with icons, currencies and bank identifiers on
  accounts.
- Budgets by account.

### Importing

- Upload bank statements and review what was imported before confirming it, with each
  statement's period and opening and closing balances kept.
- Imported transactions that duplicate existing ones are flagged.
- Importers and exporters are plugins; the CSV importer is built in, and others live in
  [pennychest-plugins](https://github.com/pennychest/pennychest-plugins).

### Categorising

- A rule engine with UK default rules, recording where each rule came from.
- Categories learned from your own history.
- AI categorisation and rule suggestions, with several providers and selectable models,
  and every AI request logged.

### Insight

- Spending and balance reports, and a dashboard of configurable widgets and charts.
- Unusual charges and recurring payments spotted for you.
- Chat with your ledger, with every change the AI makes recorded so it can be undone.

### Phone and integrations

- An installable web app that works on phones, with card taps recorded from the phone
  wallet and categorised with AI.
- An MCP server for AI assistants, signed in with personal access tokens or OAuth.

### Running it

- A single Docker image serving the API and the web app, on SQLite by default or
  PostgreSQL, with migrations run on startup.
- Password sign-in, required for every API route.
- Export the ledger through exporter plugins, and back up the database.
