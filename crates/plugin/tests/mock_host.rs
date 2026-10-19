//! Plugins written with the SDK, driven by PennyChest's side of the protocol as a mock host:
//! the example QIF importer and an exporter.

#[path = "../examples/qif/importer.rs"]
mod qif;

use std::io::{BufReader, PipeReader, PipeWriter, Write};
use std::thread::JoinHandle;

use pennychest_plugin::client::{Client, ClientError, Request};
use pennychest_plugin::protocol::{PROTOCOL_VERSION, code};
use pennychest_plugin::{
    Decimal, Exporter, ExporterInfo, Ledger, LedgerAccount, LedgerPosting, LedgerTransaction,
    NaiveDate, Plugin,
};

/// Exports the ledger as one line per posting.
struct Lines;

impl Exporter for Lines {
    fn info(&self) -> ExporterInfo {
        ExporterInfo::new("lines", "One line per posting", "txt", "text/plain")
    }

    fn export(&self, ledger: &Ledger) -> Result<Vec<u8>, String> {
        let mut out = String::new();
        for txn in &ledger.transactions {
            for posting in &txn.postings {
                let account = ledger
                    .accounts
                    .iter()
                    .find(|a| a.id == posting.account_id)
                    .ok_or("A posting's account is missing.")?;
                out.push_str(&format!(
                    "{} {} {} {}\n",
                    txn.date, account.full_path, posting.amount, txn.description
                ));
            }
        }
        Ok(out.into_bytes())
    }
}

type Host = Client<BufReader<PipeReader>, PipeWriter>;

/// A plugin serving on pipes in another thread, and the host's end of them.
fn connect(make: fn() -> Plugin) -> (Host, JoinHandle<()>) {
    let (from_host, to_plugin) = std::io::pipe().unwrap();
    let (from_plugin, to_host) = std::io::pipe().unwrap();
    let serving = std::thread::spawn(move || {
        make().serve_on(BufReader::new(from_host), to_host).unwrap();
    });
    (Client::new(BufReader::new(from_plugin), to_plugin), serving)
}

fn qif_plugin() -> Plugin {
    Plugin::new("qif", "1.0.0")
        .importer(qif::Qif)
        .exporter(Lines)
}

const STATEMENT: &str = "!Type:Bank\r\nD20/09/2026\r\nT-12.50\r\nPTESCO STORES 2041\r\n^\r\n\
                         D21/09'26\r\nT1,500.00\r\nMSalary\r\n^\r\n";

#[test]
fn the_handshake_says_what_the_plugin_provides() {
    let (mut host, serving) = connect(qif_plugin);
    let handshake = host.initialize().unwrap();
    assert_eq!(handshake.protocol_version, PROTOCOL_VERSION);
    assert_eq!(handshake.name, "qif");
    assert_eq!(handshake.importers[0].name, "qif");
    assert_eq!(handshake.importers[0].file_types, ["qif"]);
    assert!(handshake.importers[0].detects);
    assert!(!handshake.importers[0].statement_info);
    assert_eq!(handshake.exporters[0].media_type, "text/plain");
    host.shutdown().unwrap();
    serving.join().unwrap();
}

#[test]
fn the_example_importer_reads_statements() {
    let (mut host, _serving) = connect(qif_plugin);
    host.initialize().unwrap();
    assert!(
        host.detect("qif", STATEMENT.as_bytes(), "download.txt")
            .unwrap()
    );
    assert!(
        !host
            .detect("qif", b"Date,Amount\n", "statement.qif")
            .unwrap()
    );

    let rows = host.parse("qif", STATEMENT.as_bytes()).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].date, NaiveDate::from_ymd_opt(2026, 9, 20).unwrap());
    assert_eq!(rows[0].description, "TESCO STORES 2041");
    assert_eq!(rows[0].amount, "-12.50".parse::<Decimal>().unwrap());
    assert_eq!(
        rows[0].raw_content,
        "D20/09/2026\nT-12.50\nPTESCO STORES 2041"
    );
    assert_eq!(rows[0].line_number, Some(2));
    assert_eq!(rows[1].date, NaiveDate::from_ymd_opt(2026, 9, 21).unwrap());
    assert_eq!(rows[1].description, "Salary");
    assert_eq!(rows[1].amount, "1500.00".parse::<Decimal>().unwrap());

    assert_eq!(
        host.statement_info("qif", STATEMENT.as_bytes()).unwrap(),
        None
    );
}

#[test]
fn problems_with_a_file_are_told_to_the_user() {
    let (mut host, _serving) = connect(qif_plugin);
    host.initialize().unwrap();
    let error = host
        .parse("qif", b"!Type:Bank\nD31/02/2026\nT-1.00\n^\n")
        .unwrap_err();
    assert!(matches!(
        error,
        ClientError::Plugin {
            code: code::FAILED,
            ..
        }
    ));
    assert_eq!(
        error.for_user("qif"),
        "Line 2: \"31/02/2026\" isn't a date this importer reads."
    );
    let empty = host.parse("qif", b"!Type:Bank\n").unwrap_err();
    assert_eq!(empty.for_user("qif"), "The file has no transactions.");
}

#[test]
fn protocol_mistakes_are_errors_not_crashes() {
    let (mut host, _serving) = connect(qif_plugin);
    let unknown = host.parse("ofx", b"").unwrap_err();
    assert!(matches!(
        unknown,
        ClientError::Plugin {
            code: code::INVALID_PARAMS,
            ..
        }
    ));
    assert_eq!(
        unknown.for_user("qif"),
        "The qif plugin failed: No importer called ofx."
    );
    let method = host
        .call::<serde_json::Value>(Request::new("importer/sing", serde_json::json!({})))
        .unwrap_err();
    assert!(matches!(
        method,
        ClientError::Plugin {
            code: code::METHOD_NOT_FOUND,
            ..
        }
    ));
    let params = host
        .call::<serde_json::Value>(Request::new(
            "importer/parse",
            serde_json::json!({"file": "%%%"}),
        ))
        .unwrap_err();
    assert!(matches!(
        params,
        ClientError::Plugin {
            code: code::INVALID_PARAMS,
            ..
        }
    ));
    // Still answering afterwards.
    assert_eq!(host.parse("qif", STATEMENT.as_bytes()).unwrap().len(), 2);
}

#[test]
fn lines_that_are_not_requests_get_errors() {
    let plugin = qif_plugin();
    let mut output = Vec::new();
    let input = "{not json\n\n{\"jsonrpc\": \"2.0\", \"method\": 3}\n";
    plugin.serve_on(input.as_bytes(), &mut output).unwrap();
    let replies: Vec<serde_json::Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[0]["error"]["code"], code::PARSE_ERROR);
    assert_eq!(replies[1]["error"]["code"], code::INVALID_REQUEST);
}

#[test]
fn exporters_get_the_ledger_and_return_a_file() {
    let (mut host, _serving) = connect(qif_plugin);
    let account = |id, path: &str, kind: &str| LedgerAccount {
        id,
        name: path.rsplit(':').next().unwrap().into(),
        full_path: path.into(),
        kind: kind.into(),
        currency: "GBP".into(),
        parent_id: None,
        bank_identifier: None,
    };
    let posting = |id, account_id, amount: &str| LedgerPosting {
        id,
        account_id,
        amount: amount.parse().unwrap(),
    };
    let ledger = Ledger {
        today: NaiveDate::from_ymd_opt(2026, 10, 14).unwrap(),
        accounts: vec![
            account(1, "Assets:Current", "asset"),
            account(2, "Expenses:Food", "expense"),
        ],
        transactions: vec![LedgerTransaction {
            id: 1,
            date: NaiveDate::from_ymd_opt(2026, 9, 20).unwrap(),
            description: "TESCO".into(),
            status: "confirmed".into(),
            currency: "GBP".into(),
            postings: vec![posting(1, 1, "-12.5000"), posting(2, 2, "12.5000")],
        }],
        budgets: vec![],
    };
    let file = host.export("lines", &ledger).unwrap();
    assert_eq!(
        String::from_utf8(file).unwrap(),
        "2026-09-20 Assets:Current -12.5000 TESCO\n2026-09-20 Expenses:Food 12.5000 TESCO\n"
    );
}

#[test]
fn a_plugin_that_stops_is_reported() {
    let (_from_host, to_plugin) = std::io::pipe().unwrap();
    let (from_plugin, mut to_host) = std::io::pipe().unwrap();
    // A plugin that answers with nonsense, then stops.
    to_host
        .write_all(b"{\"jsonrpc\": \"2.0\", \"id\": 7}\n")
        .unwrap();
    drop(to_host);
    let mut host = Client::new(BufReader::new(from_plugin), to_plugin);
    let wrong = host.initialize().unwrap_err();
    assert!(matches!(wrong, ClientError::Malformed(_)), "{wrong}");
    assert!(matches!(
        host.initialize().unwrap_err(),
        ClientError::Closed
    ));
}
