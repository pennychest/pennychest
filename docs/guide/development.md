# Local setup

PennyChest is a Rust workspace with a React frontend. You need [Rust](https://rustup.rs) (the
version in `rust-toolchain.toml` is installed for you) and [Node.js](https://nodejs.org) 20 or
later.

| Path | What's there |
|---|---|
| `crates/core` | The ledger, imports, rules, AI, actions and taps, with no HTTP |
| `crates/server` | The `pennychest` binary: the JSON API, the MCP server and the embedded frontend |
| `crates/plugin` | The plugin protocol, and an SDK for writing plugins in Rust |
| `tools/evaluate` | Tests the learned categoriser on a database or CSV |
| `frontend` | The React app |

## Running it

The server embeds the built frontend, so build that first:

```bash
cd frontend && npm ci && npm run build && cd ..
PENNYCHEST_DATABASE_PATH=./data/pennychest.db cargo run
```

The app is at **http://localhost:8000**. On first load it offers to load a UK-oriented chart
of accounts and default categorisation rules.

While working on the frontend, run the server as above and Vite's dev server alongside it, for
hot reload. Vite sends `/api` requests to the server; set `PENNYCHEST_API_URL` if it isn't on
`http://localhost:8000`:

```bash
cd frontend && npm run dev
```

## Checks

These are what CI runs:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cd frontend && npm run lint && npm run build
```

See [Testing](../development/testing.md) for how the tests are organised.

## Building the image

```bash
docker build -t pennychest .
docker buildx build --platform linux/amd64,linux/arm64 -t pennychest .
```

The image cross-compiles, so building for arm64 on an amd64 machine needs no emulation.

## Plugins

Plugins are separate programs. See [the plugin protocol](../plugins/protocol.md) for how to
write one, in Rust with the SDK or in any other language.

## Working on these docs

```bash
pip install -r docs/requirements.txt
mkdocs serve
```

The site is served at **http://localhost:8000** by default; pass `-a localhost:8001` if the app
is already using that port.

The published site, [pennychest.github.io](https://pennychest.github.io), is built by the
[pennychest.github.io repository](https://github.com/pennychest/pennychest.github.io). Pushing
changes under `docs/` or to `mkdocs.yml` on `main` asks it to rebuild.
