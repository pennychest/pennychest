# The plugin protocol

A PennyChest plugin is a program that reads statements in a format PennyChest doesn't know
(an **importer**), or writes the ledger in another format (an **exporter**), or both.
PennyChest runs it as a separate program and talks to it over its standard input and output.
A plugin can be written in any language, can't crash PennyChest, and brings its own
dependencies.

This page is everything you need to write one. In Rust, the `pennychest-plugin` crate does
the protocol for you (see [Writing a plugin in Rust](#writing-a-plugin-in-rust)).

## How PennyChest talks to a plugin

- PennyChest starts the plugin and sends it **requests on stdin**, one per line.
- The plugin sends **one answer per request on stdout**, one per line, in the order the
  requests came.
- PennyChest starts the plugin afresh for each piece of work: `initialize`, then one request
  (or none, when it only wants the handshake), then `shutdown`. It may write all of them before
  reading any answers, so **answer each request as soon as you've read it**, without waiting
  for the next one.
- Every line is a [JSON-RPC 2.0](https://www.jsonrpc.org/specification) message, written on a
  single line (no line breaks inside it), in UTF-8.
- **stdout is only for answers.** Anything else the plugin writes there breaks the protocol.
  Write logs and debugging output to **stderr**, which PennyChest copies to its own log.
- The first request is always `initialize`. The last is `shutdown`, after which the plugin
  should exit. If PennyChest closes stdin instead, the plugin should exit too.

A request looks like this:

```json
{"jsonrpc": "2.0", "id": 1, "method": "importer/parse", "params": {"importer": "qif", "file": "IVR5cGU6QmFuawo="}}
```

and its answer like this, with the same `id`:

```json
{"jsonrpc": "2.0", "id": 1, "result": {"rows": []}}
```

### How values are written

| What | How | Example |
| --- | --- | --- |
| Files | base64 (standard alphabet, with padding) | `"IVR5cGU6QmFuawo="` |
| Dates | `YYYY-MM-DD` strings | `"2026-09-20"` |
| Amounts | decimal strings, at most four decimal places, `-` for negative | `"-12.50"` |

Amounts are **never JSON numbers**: some languages read those as floating point and lose
pennies. PennyChest refuses a row whose amount is a number.

### Errors

A request the plugin can't answer gets an error instead of a result:

```json
{"jsonrpc": "2.0", "id": 1, "error": {"code": -32000, "message": "Line 4: \"31/02/2026\" isn't a date."}}
```

| Code | Meaning |
| --- | --- |
| `-32000` | **The plugin can't do what was asked**, such as a statement it can't read. The message is shown to the user as it is, so say what's wrong in a way they can act on. |
| `-32700` | The line wasn't JSON. |
| `-32600` | The line was JSON but not a request. |
| `-32601` | The plugin doesn't have that method. |
| `-32602` | The params are wrong, e.g. a name the plugin doesn't provide. |
| `-32603` | Anything else that went wrong inside the plugin. |

Errors other than `-32000` are bugs in a plugin or in PennyChest. The user sees "The *name*
plugin failed" with the message.

### Limits

- **Size:** a message can be at most 64 MiB, which fits a file of about 48 MiB once it's
  base64. PennyChest refuses larger files before sending them.
- **Time:** a run has 10 seconds when PennyChest only wants the handshake, and 130 seconds
  when there's work to do. After that PennyChest stops the plugin and tells the user it didn't
  answer.
- A plugin that exits without answering, writes something that isn't an answer, or answers
  with the wrong `id` is reported. Each run starts afresh, so the next piece of work tries
  again.
- **Memory:** a WASI plugin has at most 1 GiB.

## Methods

### `initialize`

The handshake: PennyChest says which protocol version it speaks, and the plugin says who it is
and what it provides.

**Params:**

```json
{"protocol_version": 1}
```

**Result:**

```json
{
  "protocol_version": 1,
  "name": "qif",
  "version": "1.0.0",
  "importers": [
    {
      "name": "qif",
      "label": "QIF statements",
      "description": "Quicken Interchange Format bank and card statements",
      "file_types": ["qif"],
      "detects": true,
      "statement_info": false
    }
  ],
  "exporters": []
}
```

- `protocol_version` is the version the plugin speaks. This page describes version **1**.
  PennyChest refuses a plugin that speaks a version it doesn't.
- A plugin can provide any number of importers and exporters. Each `name` must be unique
  among everything installed, so use your plugin's name or start with it.

An **importer** is described by:

| Field | Meaning |
| --- | --- |
| `name` | Identifies it, e.g. `"hsbc"`. |
| `label` | Short name for the format, shown on the import page, e.g. `"HSBC PDF statements"`. |
| `description` | Optional, a sentence more. |
| `file_types` | Extensions it reads, without the dot, e.g. `["pdf"]`. |
| `detects` | Whether it answers `importer/detect`. If not, PennyChest picks it by extension. |
| `statement_info` | Whether it answers `importer/statement_info`. |

An **exporter** is described by:

| Field | Meaning |
| --- | --- |
| `name` | Identifies it, e.g. `"beancount"`. |
| `label` | Shown on the export page, e.g. `"Beancount ledger"`. |
| `description` | Optional, a sentence more. |
| `file_extension` | The download is named `pennychest-<date>.<file_extension>`. |
| `media_type` | e.g. `"text/plain"`. Optional, `"application/octet-stream"` by default. |

### `importer/detect`

Whether a file is one this importer reads, from its contents. Only sent to importers that say
`"detects": true`. When a file is uploaded, PennyChest asks every importer that detects, and
uses the first that says yes. If none does, it uses the first importer for the file's
extension (the built-in CSV importer last), whose `importer/parse` can then say what's wrong.

**Params:** `{"importer": "qif", "file": "<base64>", "filename": "download.txt"}`

**Result:** `true` or `false`. Keep it quick: look at the start of the file, not all of it.

### `importer/parse`

The transactions in a statement.

**Params:** `{"importer": "qif", "file": "<base64>"}`

**Result:**

```json
{
  "rows": [
    {
      "date": "2026-09-20",
      "description": "TESCO STORES 2041",
      "amount": "-12.50",
      "raw_content": "D20/09/2026\nT-12.50\nPTESCO STORES 2041",
      "line_number": 2,
      "metadata": {"reference": "A1B2"}
    }
  ]
}
```

| Field | Meaning |
| --- | --- |
| `date` | When the bank says it happened. |
| `description` | As the bank wrote it. Rules and categories match against it. |
| `amount` | Signed from the account's side: **negative for money out** (a purchase, a payment from a current account), positive for money in (salary, a refund). |
| `raw_content` | The row as the statement had it, kept as the record of what the bank reported. |
| `line_number` | Optional: where the row is in the file, for messages about it. |
| `metadata` | Optional: anything else worth keeping with the row. |

A file with no transactions, or one the importer can't read, is an error with code `-32000`
saying what's wrong (e.g. `"Line 4: \"31/02/2026\" isn't a date."`).

### `importer/statement_info`

What a statement says about its account, so the import page can suggest which account it's
for (or offer to create one) and an opening balance. Only sent to importers that say
`"statement_info": true`.

**Params:** `{"importer": "hsbc", "file": "<base64>"}`

**Result:** `null` if the file doesn't say, or:

```json
{
  "statement_type": "current_account",
  "bank_identifier": "40-12-34 12345678",
  "label": "HSBC Current Account Statement",
  "name_hint": "HSBC Advance",
  "period_start": "2026-08-21",
  "period_end": "2026-09-20",
  "opening_balance": "1520.31",
  "closing_balance": "-10.00"
}
```

| Field | Meaning |
| --- | --- |
| `statement_type` | `"current_account"`, `"savings"` or `"credit_card"`. |
| `bank_identifier` | How the statement identifies the account: `"<sort code> <account number>"` for a bank account, `"****1234"` (the card's last four digits) for a card. PennyChest matches it against its accounts. |
| `label` | What the statement is, e.g. `"HSBC Credit Card Statement"`. |
| `name_hint` | Optional: a name for a new account. |
| `period_start`, `period_end` | Optional: the dates the statement covers. |
| `opening_balance`, `closing_balance` | Optional: signed for posting, positive for money held in a bank or savings account, **negative for a credit card balance owed**. |

### `exporter/export`

The ledger in the exporter's format.

**Params:**

```json
{
  "exporter": "beancount",
  "ledger": {
    "today": "2026-10-14",
    "accounts": [
      {"id": 1, "name": "Current", "full_path": "Assets:Current", "type": "asset",
       "currency": "GBP", "parent_id": null, "bank_identifier": "40-12-34 12345678"}
    ],
    "transactions": [
      {"id": 1, "date": "2026-09-20", "description": "TESCO", "status": "confirmed",
       "currency": "GBP",
       "postings": [{"id": 1, "account_id": 1, "amount": "-12.5000"},
                    {"id": 2, "account_id": 2, "amount": "12.5000"}]}
    ],
    "budgets": [
      {"account_id": 2, "amount": "400.0000", "period": "monthly", "created_on": "2026-01-01"}
    ]
  }
}
```

- `accounts` come parents first. `type` is `"asset"`, `"liability"`, `"income"`, `"expense"`
  or `"equity"`, and `full_path` is colon-separated, e.g. `"Expenses:Food:Groceries"`.
- `transactions` come oldest first. Their postings sum to zero. `status` is `"confirmed"`, or
  `"pending"` for imported transactions still to be reviewed.
- A budget covers its account and everything below it. `period` is `"monthly"` or
  `"annual"`.

**Result:** `{"file": "<base64>"}`

### `shutdown`

**Params:** none. **Result:** `null`. The plugin then exits.

## The manifest

Every plugin comes with a `pennychest-plugin.toml`, which says what it is and how to run it
without running it:

```toml
name = "qif"                  # lowercase letters, digits, - and _
version = "1.0.0"
description = "QIF bank and card statements"
protocol = 1                  # the protocol version it speaks
kinds = ["importer"]          # "importer", "exporter" or both

[run]
wasm = "qif.wasm"             # a WASI module, or:
# command = ["./qif"]         # a program and its arguments
```

Paths are relative to the plugin's directory, and must stay inside it.

### WASI or native

- **A WASI module** (`wasm`, built for `wasm32-wasip1`) is the default, and what official
  plugins use. One `.wasm` runs on every platform. PennyChest runs it in
  [wasmtime](https://wasmtime.dev) with stdin, stdout and stderr and nothing else: no files,
  no network, no environment variables. A plugin can only see what it's sent.
- **A native program** (`command`) is for a plugin that needs something WASI can't do. It
  runs in its own directory, as the same user as PennyChest, with no environment variables
  but `PATH`. Nothing stops it reading files or using the network, which is why Settings
  warns about repositories you don't trust. It needs a build for each platform (see
  `platforms` below). The Docker image has no shell, interpreter or C library, so a native
  plugin for it must be a static executable: Rust built for `x86_64-unknown-linux-musl` or
  `aarch64-unknown-linux-musl`, Go with `CGO_ENABLED=0`, and so on.

## Publishing a plugin

Settings > Plugins installs plugins from **repositories**. A repository is a link with a
`plugins.json` at its root. For a GitHub repository that's
`https://raw.githubusercontent.com/<owner>/<repo>/HEAD/plugins.json`; any other link is taken
to be the folder holding it, or the file itself if it ends in `.json`. The official repository
is [pennychest-plugins](https://github.com/pennychest/pennychest-plugins).

```json
{
  "name": "My PennyChest plugins",
  "plugins": [
    {
      "package": "qif",
      "name": "QIF statements",
      "description": "Import QIF statements from most banks",
      "version": "1.0.0",
      "url": "https://github.com/me/my-plugins/releases/download/qif-v1.0.0/qif-1.0.0.tar.gz",
      "sha256": "<the archive's SHA-256, as 64 hex digits>"
    }
  ]
}
```

- `package` is the plugin's `name` from its manifest.
- `url` is an `https://` link to a `.tar.gz` of the plugin's directory: the manifest and the
  files it names, either at the top of the archive or in one directory there.
- PennyChest checks the download against `sha256` before unpacking anything. It starts the
  plugin once to check it works, and only then replaces the installed version.
- A native plugin lists a download for each platform it's built for, in place of `url` and
  `sha256`. The platform is the operating system and CPU, as Rust names them:

    ```json
    "platforms": {
      "linux-x86_64": { "url": "https://…", "sha256": "…" },
      "linux-aarch64": { "url": "https://…", "sha256": "…" }
    }
    ```

To make an archive and its checksum:

```sh
tar -czf qif-1.0.0.tar.gz qif/
sha256sum qif-1.0.0.tar.gz
```

Installed plugins live in `PENNYCHEST_PLUGIN_DIR` (`/data/plugins` in Docker), one directory
each. They're loaded when PennyChest starts, and straight away after installing from Settings.
You can also copy a plugin's directory there yourself, and restart.

## Writing a plugin in Rust

Add `pennychest-plugin` as a dependency, implement `Importer` or `Exporter`, and serve:

```rust
use pennychest_plugin::{Importer, ImporterInfo, Plugin, Row};

struct Qif;

impl Importer for Qif {
    fn info(&self) -> ImporterInfo {
        ImporterInfo::new("qif", "QIF statements", &["qif"]).detects()
    }

    fn detect(&self, file: &[u8], _filename: &str) -> bool {
        file.starts_with(b"!Type:")
    }

    fn parse(&self, file: &[u8]) -> Result<Vec<Row>, String> {
        // An Err is shown to the user, so say what's wrong with the file.
        todo!()
    }
}

fn main() {
    Plugin::new("qif", env!("CARGO_PKG_VERSION")).importer(Qif).serve();
}
```

The SDK handles the protocol: the handshake, base64, decimal amounts, errors and shutdown.
The whole example is [`crates/plugin/examples/qif`](https://github.com/pennychest/pennychest/tree/main/crates/plugin/examples/qif). Build it
for WASI with `cargo build --example qif --target wasm32-wasip1`, or natively without
`--target`.

## Writing a plugin in another language

Read lines from stdin, answer each on stdout, and flush after each answer.

## Testing a plugin

Send it requests by hand:

```sh
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocol_version":1}}' \
  '{"jsonrpc":"2.0","id":2,"method":"shutdown"}' | ./your-plugin
```

In Rust, `pennychest_plugin::client::Client` is PennyChest's own side of the protocol: connect
it to your plugin, as [`crates/plugin/tests/mock_host.rs`](https://github.com/pennychest/pennychest/blob/main/crates/plugin/tests/mock_host.rs)
does, and call `initialize`, `parse` and the rest.
