//! The plugin host: running plugins as WASI modules and native programs, behind the
//! importer and exporter traits, and installing and removing them.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use pennychest_core::export::Exporters;
use pennychest_core::imports::Importers;
use pennychest_core::plugins::install::InstallError;
use pennychest_core::plugins::{Plugins, Timeouts};
use pennychest_plugin::{Ledger, NaiveDate};
use sha2::{Digest, Sha256};

const QIF: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/plugins/qif");

const STATEMENT: &[u8] = b"!Type:Bank\nD20/09/2026\nT-12.50\nPTESCO STORES 2041\n^\n\
D21/09/2026\nT1500.00\nPSALARY\n^\n";

/// A plugin directory holding a copy of the QIF plugin.
fn with_qif() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    copy_dir(Path::new(QIF), &dir.path().join("qif"));
    dir
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
    }
}

/// The QIF plugin's directory as a `.tar.gz`, as a repository serves it.
fn qif_archive(manifest: Option<&str>) -> Vec<u8> {
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    let text = manifest.map_or_else(
        || std::fs::read_to_string(Path::new(QIF).join("pennychest-plugin.toml")).unwrap(),
        str::to_owned,
    );
    let mut header = tar::Header::new_gnu();
    header.set_size(text.len() as u64);
    header.set_mode(0o644);
    builder
        .append_data(&mut header, "qif/pennychest-plugin.toml", text.as_bytes())
        .unwrap();
    builder
        .append_path_with_name(Path::new(QIF).join("qif.wasm"), "qif/qif.wasm")
        .unwrap();
    builder.into_inner().unwrap().finish().unwrap()
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[test]
fn a_wasi_plugin_imports_like_a_built_in_importer() {
    let dir = with_qif();
    let plugins = Plugins::load(dir.path());
    assert!(plugins.broken().is_empty(), "{:?}", plugins.broken());
    assert_eq!(plugins.installed()[0].name, "qif");

    let importers = plugins.importers(&Importers::default());
    let names: Vec<String> = importers.list().into_iter().map(|i| i.name).collect();
    assert_eq!(names, ["csv", "qif"]);
    assert_eq!(importers.supported_extensions(), [".csv", ".qif", ".txt"]);

    // Recognised from its contents, whatever it's called.
    let found = importers.find(STATEMENT, "download.txt").unwrap();
    assert_eq!(found.name(), "qif");
    let rows = found.parse(STATEMENT).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].description, "TESCO STORES 2041");
    assert_eq!(rows[0].amount.to_string(), "-12.5000");
    assert_eq!(rows[0].date, NaiveDate::from_ymd_opt(2026, 9, 20).unwrap());
    assert_eq!(rows[1].amount.to_string(), "1500.0000");
}

#[test]
fn a_plugins_own_errors_reach_the_user_as_it_wrote_them() {
    let dir = with_qif();
    let plugins = Plugins::load(dir.path());
    let qif = plugins.importers(&Importers::default()).get("qif").unwrap();
    let error = qif
        .parse(b"!Type:Bank\nD31/02/2026\nT-1.00\n^\n")
        .unwrap_err();
    assert!(error.contains("31/02/2026"), "{error}");
    assert!(!error.contains("plugin failed"), "{error}");
}

#[test]
fn other_things_in_the_plugin_directory_are_left_alone() {
    let dir = with_qif();
    std::fs::create_dir_all(dir.path().join("cache/downloads")).unwrap();
    std::fs::write(dir.path().join("notes.txt"), "hello").unwrap();
    let plugins = Plugins::load(dir.path());
    assert_eq!(plugins.installed().len(), 1);
    assert!(plugins.broken().is_empty());
}

#[test]
fn a_plugin_that_wont_load_is_listed_as_broken() {
    let dir = with_qif();
    std::fs::write(dir.path().join("qif/qif.wasm"), b"not wasm").unwrap();
    let plugins = Plugins::load(dir.path());
    assert!(plugins.installed().is_empty());
    let [broken] = &plugins.broken()[..] else {
        panic!("{:?}", plugins.broken())
    };
    assert_eq!(broken.name, "qif");
    assert!(broken.problem.contains("WebAssembly"), "{}", broken.problem);
    assert_eq!(plugins.importers(&Importers::default()).list().len(), 1);
}

#[test]
fn a_missing_plugin_directory_means_no_plugins() {
    let plugins = Plugins::load(Path::new("/nonexistent/plugins"));
    assert!(plugins.installed().is_empty() && plugins.broken().is_empty());
}

#[test]
fn installs_updates_and_removes_a_plugin() {
    let dir = tempfile::tempdir().unwrap();
    let plugins = Plugins::load(&dir.path().join("plugins"));
    let archive = qif_archive(None);
    let info = plugins.install("qif", &archive, &sha256(&archive)).unwrap();
    assert_eq!(info.version, "0.1.0");
    // Picked up straight away, without a restart.
    let importers = plugins.importers(&Importers::default());
    assert_eq!(importers.find(STATEMENT, "x.qif").unwrap().name(), "qif");

    let manifest = std::fs::read_to_string(Path::new(QIF).join("pennychest-plugin.toml"))
        .unwrap()
        .replace("version = \"0.1.0\"", "version = \"0.2.0\"");
    let update = qif_archive(Some(&manifest));
    plugins.install("qif", &update, &sha256(&update)).unwrap();
    assert_eq!(plugins.installed()[0].version, "0.2.0");
    let leftovers: Vec<PathBuf> = std::fs::read_dir(dir.path().join("plugins"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(leftovers, [dir.path().join("plugins/qif")]);

    plugins.uninstall("qif").unwrap();
    assert!(plugins.installed().is_empty());
    assert!(
        plugins
            .importers(&Importers::default())
            .get("qif")
            .is_none()
    );
    assert!(matches!(
        plugins.uninstall("qif"),
        Err(InstallError::NotInstalled)
    ));
}

#[test]
fn wont_install_what_the_repository_didnt_promise() {
    let dir = tempfile::tempdir().unwrap();
    let plugins = Plugins::load(dir.path());
    let archive = qif_archive(None);
    let error = plugins
        .install("qif", &archive, &sha256(b"something else"))
        .unwrap_err();
    assert!(matches!(error, InstallError::Checksum), "{error}");
    let error = plugins
        .install("hsbc", &archive, &sha256(&archive))
        .unwrap_err();
    assert!(
        error.to_string().contains("it's the qif plugin, not hsbc"),
        "{error}"
    );
    assert!(
        std::fs::read_dir(dir.path()).unwrap().next().is_none(),
        "nothing left behind"
    );
}

#[test]
fn a_broken_update_leaves_the_installed_version() {
    let dir = with_qif();
    let plugins = Plugins::load(dir.path());
    let manifest = std::fs::read_to_string(Path::new(QIF).join("pennychest-plugin.toml"))
        .unwrap()
        .replace("qif.wasm", "missing.wasm");
    let broken = qif_archive(Some(&manifest));
    let error = plugins
        .install("qif", &broken, &sha256(&broken))
        .unwrap_err();
    assert!(matches!(error, InstallError::Broken(_)), "{error}");
    assert_eq!(plugins.installed().len(), 1);
    assert!(dir.path().join("qif/qif.wasm").exists());
}

/// A native plugin: a shell script that gives the same answers whatever it's asked, in the
/// order PennyChest asks (the handshake, the work, then shutdown).
#[cfg(unix)]
fn script_plugin(dir: &Path, answers: &[&str], before: &str) {
    use std::os::unix::fs::PermissionsExt;
    let plugin = dir.join("canned");
    std::fs::create_dir_all(&plugin).unwrap();
    std::fs::write(
        plugin.join("pennychest-plugin.toml"),
        "name = \"canned\"\nversion = \"1.0.0\"\nprotocol = 1\nkinds = [\"exporter\"]\n\
         [run]\ncommand = [\"run.sh\"]\n",
    )
    .unwrap();
    let mut script = format!("#!/bin/sh\n{before}\n");
    for answer in answers {
        script.push_str(&format!(
            "read -r _\necho '{answer}'\necho 'to the log' >&2\n"
        ));
    }
    std::fs::write(plugin.join("run.sh"), script).unwrap();
    std::fs::set_permissions(
        plugin.join("run.sh"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
}

#[cfg(unix)]
const CANNED_HANDSHAKE: &str = r#"{"jsonrpc":"2.0","id":1,"result":{"protocol_version":1,"name":"canned","version":"1.0.0","exporters":[{"name":"canned","label":"Canned","file_extension":"txt","media_type":"text/plain"}]}}"#;

#[cfg(unix)]
#[test]
fn a_native_plugin_exports() {
    let dir = tempfile::tempdir().unwrap();
    script_plugin(
        dir.path(),
        &[
            CANNED_HANDSHAKE,
            r#"{"jsonrpc":"2.0","id":2,"result":{"file":"aGVsbG8="}}"#,
            r#"{"jsonrpc":"2.0","id":3,"result":null}"#,
        ],
        "",
    );
    let plugins = Plugins::load(dir.path());
    assert!(plugins.broken().is_empty(), "{:?}", plugins.broken());
    let exporters = plugins.exporters(&Exporters::default());
    let canned = exporters.get("canned").unwrap();
    assert_eq!(canned.media_type(), "text/plain");
    let ledger = Ledger {
        today: NaiveDate::from_ymd_opt(2026, 10, 14).unwrap(),
        accounts: vec![],
        transactions: vec![],
        budgets: vec![],
    };
    assert_eq!(canned.export(&ledger).unwrap(), b"hello");
}

#[cfg(unix)]
#[test]
fn a_plugin_that_takes_too_long_is_stopped() {
    let dir = tempfile::tempdir().unwrap();
    script_plugin(dir.path(), &[CANNED_HANDSHAKE], "sleep 30");
    let timeouts = Timeouts {
        handshake: Duration::from_millis(500),
        call: Duration::from_millis(500),
    };
    let started = Instant::now();
    let plugins = Plugins::load_with(dir.path(), timeouts);
    assert!(started.elapsed() < Duration::from_secs(10));
    let problem = &plugins.broken()[0].problem;
    assert!(problem.contains("didn't answer"), "{problem}");
}

#[cfg(unix)]
#[test]
fn a_plugin_that_breaks_the_protocol_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    script_plugin(dir.path(), &["this isn't JSON"], "");
    let plugins = Plugins::load(dir.path());
    let problem = &plugins.broken()[0].problem;
    assert!(problem.contains("The canned plugin failed"), "{problem}");
}
