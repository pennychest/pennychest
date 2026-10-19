//! Typed operations on the user's data, shared by the in-app chat, the MCP server and
//! `/api/actions`.
//!
//! Each action declares its input as a Rust type, whose JSON schema (from `schemars`) is what
//! AI models are given, so the two can't drift apart, and the scope a caller needs to run it.
//! Results are JSON built from database queries, so numbers never come from a model doing
//! arithmetic. Every change an action makes is recorded with what's needed to undo it (see
//! `changes`).

/// Declares an action named after its handler, `async fn(&SqlitePool, Input) -> Result<Output,
/// ActionError>`.
macro_rules! action {
    ($handler:ident, $input:ty, $scope:expr, $description:expr) => {
        Action {
            name: stringify!($handler),
            description: $description,
            scope: $scope,
            schema: schema::<$input>,
            run: |db, arguments| {
                Box::pin(async move {
                    let input: $input = parse(arguments)?;
                    $handler(&db, input).await
                })
            },
        }
    };
}

pub mod changes;
mod common;
mod insights;
mod ledger;
mod read;
mod undo;
mod write;

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::LazyLock;

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sqlx::SqlitePool;

use crate::accounts::{FindError, quoted};
use crate::reports::ReportError;

pub use undo::UndoError;

/// What a caller is allowed to do. Read is always granted; the others add to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Read,
    /// Categories, rules and budgets.
    Organise,
    /// The ledger itself.
    Transactions,
}

impl Scope {
    pub const ALL: [Self; 3] = [Self::Read, Self::Organise, Self::Transactions];

    pub fn name(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Organise => "organise",
            Self::Transactions => "transactions",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Read => "Look up your data",
            Self::Organise => "Change categories, rules and budgets",
            Self::Transactions => "Change transactions",
        }
    }
}

impl FromStr for Scope {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|scope| scope.name() == s)
            .ok_or_else(|| format!("{} isn't a permission", quoted(s)))
    }
}

/// Who made a change, as the change log shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "lowercase")]
#[sqlx(type_name = "TEXT", rename_all = "lowercase")]
pub enum Source {
    Chat,
    Api,
    Mcp,
    /// Categorising an import as it arrives.
    Import,
}

/// A problem with an action call, worded so an AI model or a person can fix the call.
#[derive(Debug, thiserror::Error)]
pub enum ActionError {
    #[error("There is no action called {}.", quoted(.0))]
    Unknown(String),
    #[error("{0}")]
    NotAllowed(String),
    #[error("{0}")]
    Invalid(String),
    /// Input that doesn't fit the action's schema, before it's prefixed with the action.
    #[error("{0}")]
    Input(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

impl ActionError {
    fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }
}

impl From<FindError> for ActionError {
    fn from(error: FindError) -> Self {
        match error {
            FindError::NotFound(message) | FindError::Ambiguous(message) => Self::Invalid(message),
            FindError::Database(error) => Self::Database(error),
        }
    }
}

impl From<ReportError> for ActionError {
    fn from(error: ReportError) -> Self {
        match error {
            ReportError::Invalid(message) | ReportError::UnknownCategory(message) => {
                Self::Invalid(message)
            }
            ReportError::Database(error) => Self::Database(error),
        }
    }
}

/// What an action did: its result, and for a change, a summary and how to undo it.
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub result: Map<String, Value>,
    pub change: Option<Change>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub summary: String,
    /// What the undo for this action needs, as `undo` reads it.
    pub undo: Value,
}

impl Output {
    fn new(result: Value) -> Self {
        let Value::Object(result) = result else {
            panic!("an action's result is a JSON object");
        };
        Self {
            result,
            change: None,
        }
    }

    fn with_change(mut self, summary: impl Into<String>, undo: impl Serialize) -> Self {
        self.change = Some(Change {
            summary: summary.into(),
            undo: serde_json::to_value(undo).expect("undo data serialises"),
        });
        self
    }
}

type Running = Pin<Box<dyn Future<Output = Result<Output, ActionError>> + Send>>;

pub struct Action {
    pub name: &'static str,
    pub description: &'static str,
    pub scope: Scope,
    schema: fn() -> Value,
    run: fn(SqlitePool, Value) -> Running,
}

impl Action {
    /// The JSON schema of the action's input, as AI models are given it.
    pub fn input_schema(&self) -> Value {
        (self.schema)()
    }
}

impl fmt::Debug for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Action").field("name", &self.name).finish()
    }
}

/// Input checks beyond the input's shape, such as ranges, for the `Input` error.
trait Check {
    fn check(&self) -> Result<(), String> {
        Ok(())
    }
}

fn schema<T: JsonSchema>() -> Value {
    let generator = schemars::generate::SchemaSettings::draft2020_12()
        .with(|s| s.inline_subschemas = true)
        .into_generator();
    let mut schema =
        serde_json::to_value(generator.into_root_schema_for::<T>()).expect("schemas serialise");
    if let Value::Object(map) = &mut schema {
        map.remove("$schema");
        map.remove("title");
        map.remove("description");
        // Inputs with no fields still say they're objects that take nothing else.
        map.entry("properties").or_insert_with(|| json!({}));
        if let Some(Value::Object(properties)) = map.get_mut("properties") {
            for property in properties.values_mut() {
                tidy(property);
            }
        }
    }
    schema
}

/// Joins a description's wrapped lines, as doc comments wrap them.
fn tidy(property: &mut Value) {
    if let Some(Value::String(text)) = property.get_mut("description") {
        *text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    }
}

fn parse<T: DeserializeOwned + Check>(arguments: Value) -> Result<T, ActionError> {
    let arguments = if arguments.is_null() {
        json!({})
    } else {
        arguments
    };
    let input: T = serde_path_to_error::deserialize(arguments).map_err(|e| {
        let path = e.path().to_string();
        let message = e.inner().to_string();
        ActionError::Input(if path == "." {
            message
        } else {
            format!("{path}: {message}")
        })
    })?;
    input.check().map_err(ActionError::Input)?;
    Ok(input)
}

static ACTIONS: LazyLock<Vec<Action>> = LazyLock::new(|| {
    let mut actions = read::actions();
    actions.extend(insights::actions());
    actions.extend(write::actions());
    actions.extend(ledger::actions());
    actions
});

/// Every action, in the order they're offered.
pub fn catalog() -> &'static [Action] {
    &ACTIONS
}

pub fn find(name: &str) -> Option<&'static Action> {
    ACTIONS.iter().find(|a| a.name == name)
}

fn granted(scopes: &[Scope]) -> impl Fn(Scope) -> bool + '_ {
    move |scope| scope == Scope::Read || scopes.contains(&scope)
}

/// The actions a caller with these scopes may run (read-only with none), as
/// `/api/actions` lists them.
pub fn describe(scopes: &[Scope]) -> Vec<Value> {
    let allowed = granted(scopes);
    ACTIONS
        .iter()
        .filter(|a| allowed(a.scope))
        .map(|a| {
            json!({
                "name": a.name,
                "description": a.description,
                "scope": a.scope,
                "input_schema": a.input_schema(),
            })
        })
        .collect()
}

/// Who's running an action and on whose behalf.
#[derive(Debug, Clone, Copy)]
pub struct Caller<'a> {
    /// What it may do besides read. Callers are read-only unless they pass the scopes they
    /// hold, so one that forgets to check its permissions can only fail safe.
    pub scopes: &'a [Scope],
    pub source: Source,
    pub conversation_id: Option<i64>,
}

impl Caller<'_> {
    /// A signed-in session, which has full access to the user's data.
    pub const SESSION: Caller<'static> = Caller {
        scopes: &Scope::ALL,
        source: Source::Api,
        conversation_id: None,
    };
}

/// Runs an action if the caller's scopes allow it. A change it makes is recorded, and the
/// result says so under "change" with the change's id and summary.
pub async fn run(
    db: &SqlitePool,
    name: &str,
    arguments: Value,
    caller: Caller<'_>,
) -> Result<Map<String, Value>, ActionError> {
    let action = find(name).ok_or_else(|| ActionError::Unknown(name.to_owned()))?;
    if !granted(caller.scopes)(action.scope) {
        return Err(ActionError::NotAllowed(format!(
            "{name} needs the {} permission ({}), which this caller doesn't have.",
            quoted(action.scope.name()),
            action.scope.label().to_lowercase()
        )));
    }
    let recorded_arguments = arguments.clone();
    let output = (action.run)(db.clone(), arguments)
        .await
        .map_err(|e| match e {
            ActionError::Input(problem) => {
                ActionError::Invalid(format!("Invalid input for {name}: {problem}"))
            }
            other => other,
        })?;
    let mut result = output.result;
    if let Some(change) = output.change {
        let id = changes::record(
            db,
            &changes::NewChange {
                action: name,
                arguments: Some(&recorded_arguments),
                summary: &change.summary,
                undo: &change.undo,
                source: caller.source,
                conversation_id: caller.conversation_id,
            },
        )
        .await?;
        result.insert(
            "change".into(),
            json!({ "id": id, "summary": change.summary }),
        );
    }
    Ok(result)
}

// Checks for inputs, worded like the schema's limits.

fn in_range(field: &str, value: i64, min: i64, max: i64) -> Result<(), String> {
    if value < min {
        Err(format!("{field}: must be at least {min}"))
    } else if value > max {
        Err(format!("{field}: must be at most {max}"))
    } else {
        Ok(())
    }
}

fn length(field: &str, text: &str, min: usize, max: usize) -> Result<(), String> {
    let count = text.chars().count();
    if count < min {
        Err(format!("{field}: must have at least {min} characters"))
    } else if count > max {
        Err(format!("{field}: must have at most {max} characters"))
    } else {
        Ok(())
    }
}

fn items<T>(field: &str, list: &[T], min: usize, max: usize) -> Result<(), String> {
    if list.len() < min {
        Err(format!("{field}: must have at least {min} item"))
    } else if list.len() > max {
        Err(format!("{field}: must have at most {max} items"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_input_schema_is_a_closed_object() {
        for action in catalog() {
            let schema = action.input_schema();
            assert_eq!(schema["type"], "object", "{}", action.name);
            assert_eq!(schema["additionalProperties"], false, "{}", action.name);
            assert!(schema.get("title").is_none(), "{}", action.name);
            assert!(!action.description.trim().is_empty(), "{}", action.name);
        }
    }

    #[test]
    fn every_changing_action_can_be_undone() {
        let mut changing: Vec<&str> = catalog()
            .iter()
            .filter(|a| a.scope != Scope::Read)
            .map(|a| a.name)
            .collect();
        let mut undoable = undo::UNDOABLE.to_vec();
        changing.sort_unstable();
        undoable.sort_unstable();
        assert_eq!(changing, undoable);
    }

    #[test]
    fn scopes_parse_by_name() {
        assert_eq!("organise".parse::<Scope>(), Ok(Scope::Organise));
        assert_eq!(
            "admin".parse::<Scope>(),
            Err("'admin' isn't a permission".into())
        );
    }
}
