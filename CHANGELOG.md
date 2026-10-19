# Changelog

All notable changes to this project are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Until 1.0.0 the API, database schema and plugin interface may still change between minor
versions; anything breaking will be called out here.

## v0.5.1 (2026-10-19)

### Changed

- 🐳 Build both platforms without the cache mounts colliding (#9)

## v0.5.0 (2026-10-19)

### Added

- ✨ PennyChest in Rust (#8)

### Documentation

- 📝 Show the screenshots on the docs home page, and suggest next steps after the quick start (#7)
- 📝 Return to the minimal README, with the screenshots (#6)
- 📝 Product README, one-click deploy and plugin docs (#5)

## v0.4.0 (2026-10-10)

### Added

- ✨ Map a CSV's columns with AI and save the mapping as a template (#4)

## v0.3.0 (2026-10-10)

### Added

- ✨ Split AI settings into a decision model and a language model, with each task on demand or automatic (#3)

## v0.2.0 (2026-10-10)

### Added

- ✨ Install plugins from Settings, from the official repository or ones you add (#2)

## v0.1.1 (2026-10-04)

### Changed

- 🔧 Only use Jev's categories when it's at least 90% sure

### Documentation

- 📝 Document categorisation, card taps, AI, MCP and deploying to Fly.io

## v0.1.0 (2026-10-04)

### Added

- 📦 Export the ledger through exporter plugins, and back up the database
- 🧠 Learn categories from your own history
- ✨ Spot unusual charges and recurring payments
- 🔐 Let MCP connectors sign in with OAuth
- 🔐 Add an MCP server with personal access tokens
- ✨ Categorise card taps with AI
- 💬 Chat with your ledger
- ✨ Add a catalogue of ledger actions
- 📊 Add dashboard widgets and charts
- ✨ Add the dashboard API
- ✨ Add budgets by account
- 📱 Record card taps from the phone wallet
- ✨ Link both sides of transfers between accounts
- ✨ Keep each statement's period and balances
- 🤖 Categorise transactions and suggest rules with AI
- ✨ Store app settings and AI provider configuration
- 🧭 Add the app layout, navigation and routes
- ✨ Add a typed API client
- 📈 Add spending and balance reports
- ✨ Upload statements and review imported transactions
- 🔌 Add a plugin interface for importers, with a CSV importer
- ✨ Add the rule engine with UK default rules
- ✨ Record transactions as balanced double-entry postings
- ✨ Add the accounts API

### Fixed

- 🐛 Flag imported transactions that duplicate existing ones
- 🔒 Require a password and signed-in session

### Changed

- 🔧 Report the package version from the API and MCP server
- 🐳 Build the frontend natively when building the image for another platform
- 💄 Fit the rules header and dashboard editing on phones
- ⏪ Record AI changes so they can be undone
- ♻️ Support several AI providers and selectable models
- 💄 Add the card taps page
- 💄 Add the settings page with AI provider settings
- 🔊 Log AI requests
- 🗃️ Record where each rule came from
- 💄 Add the rules page
- 💄 Add the import and review pages
- 💄 Add the transactions page
- 💄 Add the accounts page
- 💄 Add the shared UI components
- 🍱 Add the logo, app icons and PWA manifest
- 🐳 Run the app with Docker and Compose
- 🗃️ Track currency on transactions and icons on accounts
- 🗃️ Store bank identifiers and import metadata
- 🗃️ Add tables for import batches and categorisation rules
- 🗃️ Add the accounts, transactions and postings schema
- 🔧 Set up the FastAPI backend and database connection

### Documentation

- 📝 Move detailed docs into an MkDocs site
- 📝 Write the README
