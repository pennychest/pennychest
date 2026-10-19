//! `pennychest-plugin.toml`, which describes a plugin before it's run: what it is, what it
//! provides and how to start it.
//!
//! ```toml
//! name = "hsbc"
//! version = "1.2.0"
//! description = "HSBC PDF statements"
//! protocol = 1
//! kinds = ["importer"]
//!
//! [run]
//! wasm = "hsbc.wasm"        # a WASI module
//! # or: command = ["hsbc"]  # a program, with any arguments
//! ```

use serde::Deserialize;

use crate::protocol::PROTOCOL_VERSION;

pub const FILE_NAME: &str = "pennychest-plugin.toml";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Importer,
    Exporter,
}

/// How to start the plugin. Paths are relative to the plugin's directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Run {
    /// A WASI module, run in a sandbox.
    Wasm(String),
    /// A program and its arguments.
    Command(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub description: String,
    pub protocol: u32,
    pub kinds: Vec<Kind>,
    pub run: Run,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunTable {
    wasm: Option<String>,
    command: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    name: String,
    version: String,
    #[serde(default)]
    description: String,
    protocol: u32,
    kinds: Vec<Kind>,
    run: RunTable,
}

/// Names that are safe as a directory name and in a URL: lowercase letters, digits, `-` and
/// `_`, at most 64 characters.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

impl Manifest {
    /// Reads a manifest, saying what's wrong with one PennyChest can't use.
    pub fn parse(text: &str) -> Result<Self, String> {
        let file: File = toml::from_str(text).map_err(|e| format!("{FILE_NAME}: {e}"))?;
        if !valid_name(&file.name) {
            return Err(format!(
                "{FILE_NAME}: the name must be lowercase letters, digits, - and _"
            ));
        }
        if file.protocol != PROTOCOL_VERSION {
            return Err(format!(
                "{FILE_NAME}: the plugin speaks protocol version {}, but PennyChest speaks \
                 version {PROTOCOL_VERSION}",
                file.protocol
            ));
        }
        if file.kinds.is_empty() {
            return Err(format!(
                "{FILE_NAME}: kinds must name importer, exporter or both"
            ));
        }
        let run = match (file.run.wasm, file.run.command) {
            (Some(wasm), None) if !wasm.is_empty() => Run::Wasm(wasm),
            (None, Some(command)) if !command.is_empty() => Run::Command(command),
            _ => {
                return Err(format!(
                    "{FILE_NAME}: [run] must give either wasm or command"
                ));
            }
        };
        Ok(Self {
            name: file.name,
            version: file.version,
            description: file.description,
            protocol: file.protocol,
            kinds: file.kinds,
            run,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HSBC: &str = r#"
        name = "hsbc"
        version = "1.2.0"
        description = "HSBC PDF statements"
        protocol = 1
        kinds = ["importer"]

        [run]
        wasm = "hsbc.wasm"
    "#;

    #[test]
    fn reads_a_manifest() {
        let manifest = Manifest::parse(HSBC).unwrap();
        assert_eq!(manifest.name, "hsbc");
        assert_eq!(manifest.kinds, [Kind::Importer]);
        assert_eq!(manifest.run, Run::Wasm("hsbc.wasm".into()));
        let native = HSBC.replace(
            r#"wasm = "hsbc.wasm""#,
            r#"command = ["./hsbc", "--quiet"]"#,
        );
        assert_eq!(
            Manifest::parse(&native).unwrap().run,
            Run::Command(vec!["./hsbc".into(), "--quiet".into()])
        );
    }

    #[test]
    fn says_what_is_wrong() {
        let cases = [
            (
                HSBC.replace("\"hsbc\"\n", "\"HSBC Bank\"\n"),
                "the name must be",
            ),
            (
                HSBC.replace("protocol = 1", "protocol = 9"),
                "protocol version 9",
            ),
            (HSBC.replace(r#"["importer"]"#, "[]"), "kinds must name"),
            (
                HSBC.replace(r#"["importer"]"#, r#"["scraper"]"#),
                "unknown variant",
            ),
            (
                HSBC.replace(
                    r#"wasm = "hsbc.wasm""#,
                    r#"wasm = "a.wasm"
command = ["b"]"#,
                ),
                "either wasm or command",
            ),
            (
                HSBC.replace("version = \"1.2.0\"\n", ""),
                "missing field `version`",
            ),
        ];
        for (text, expected) in cases {
            let problem = Manifest::parse(&text).unwrap_err();
            assert!(problem.contains(expected), "{problem}");
        }
    }
}
