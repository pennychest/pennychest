//! An importer plugin for QIF statements, written with the SDK.
//!
//! Build it natively with `cargo build --example qif`, or for WASI with
//! `cargo build --example qif --target wasm32-wasip1`.

mod importer;

use pennychest_plugin::Plugin;

fn main() {
    Plugin::new("qif", env!("CARGO_PKG_VERSION"))
        .importer(importer::Qif)
        .serve();
}
