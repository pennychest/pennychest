//! Actions that organise: categories, rules and budgets, and moving transactions between
//! categories.

use rust_decimal::Decimal;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{SqliteConnection, SqlitePool};

use super::common::{self, UNCATEGORISED_PATH, count_of, unique};
use super::{Action, ActionError, Check, Output, Scope, in_range, items, length, parse, schema};
use crate::Money;
use crate::accounts::{self, AccountType, NewAccount, path_segment, quoted};
use crate::budgets::{self, BudgetError, Period};
use crate::lookups::{CategorisationSource, MatchType};
use crate::rules::{self, NewRule};

pub fn actions() -> Vec<Action> {
    vec![
        action!(
            create_category,
            CreateCategoryInput,
            Scope::Organise,
            "Create a new spending or income category under an existing one. It takes the \
             parent's type, so a category under \"Expenses\" is for spending."
        ),
        action!(
            rename_category,
            RenameCategoryInput,
            Scope::Organise,
            "Rename a category. Its subcategories, budgets, rules and transactions stay \
             attached."
        ),
        action!(
            create_rule,
            CreateRuleInput,
            Scope::Organise,
            "Create a rule that automatically categorises future imported transactions whose \
             description matches the pattern. It doesn't change existing transactions; use \
             recategorise_transactions for those."
        ),
        action!(
            recategorise_transactions,
            RecategoriseInput,
            Scope::Transactions,
            "Move transactions to a different category. Transactions split across several \
             categories, or with no category (such as transfers between the user's own \
             accounts), are skipped and reported."
        ),
        action!(
            set_budget,
            SetBudgetInput,
            Scope::Organise,
            "Create a budget for a spending category, or change the limit if it already has one \
             for that period. A category can have both a monthly and an annual budget."
        ),
        action!(
            delete_budget,
            DeleteBudgetInput,
            Scope::Organise,
            "Remove a category's monthly or annual budget."
        ),
    ]
}

async fn path_taken(db: &SqlitePool, path: &str) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM accounts WHERE full_path = ?)")
        .bind(path)
        .fetch_one(db)
        .await
}

/// Gives an account a new path and name, carrying its subcategories along. Returns how many
/// subcategories moved.
pub async fn move_account(
    db: &mut SqliteConnection,
    id: i64,
    old_path: &str,
    new_path: &str,
    new_name: &str,
) -> Result<u64, sqlx::Error> {
    let moved = sqlx::query(
        "UPDATE accounts SET full_path = ?1 || substr(full_path, length(?2) + 1)
         WHERE substr(full_path, 1, length(?2) + 1) = ?2 || ':'",
    )
    .bind(new_path)
    .bind(old_path)
    .execute(&mut *db)
    .await?
    .rows_affected();
    sqlx::query("UPDATE accounts SET full_path = ?, name = ? WHERE id = ?")
        .bind(new_path)
        .bind(new_name)
        .bind(id)
        .execute(&mut *db)
        .await?;
    Ok(moved)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateCategoryInput {
    /// Display name, e.g. "Amazon Prime".
    #[schemars(length(min = 1, max = 60))]
    name: String,
    /// The category to create it under (path or name), e.g. "Expenses:Entertainment" or
    /// "Income".
    parent: String,
}

impl Check for CreateCategoryInput {
    fn check(&self) -> Result<(), String> {
        length("name", &self.name, 1, 60)
    }
}

#[derive(Serialize, Deserialize)]
pub struct CreatedCategory {
    pub account_id: i64,
}

pub async fn create_category(
    db: &SqlitePool,
    input: CreateCategoryInput,
) -> Result<Output, ActionError> {
    let parent = accounts::find_category(db, &input.parent).await?;
    let segment = path_segment(&input.name);
    if segment.is_empty() {
        return Err(ActionError::invalid(
            "The name needs at least one letter or number.",
        ));
    }
    let path = format!("{}:{segment}", parent.full_path);
    if path_taken(db, &path).await? {
        return Err(ActionError::Invalid(format!(
            "A category {} already exists.",
            quoted(&path)
        )));
    }
    let account = accounts::create(
        db,
        NewAccount {
            name: input.name.trim().to_owned(),
            full_path: path,
            parent_id: Some(parent.id),
            kind: parent.kind,
            currency: parent.currency.clone(),
            bank_identifier: None,
            icon: None,
        },
    )
    .await
    .map_err(|e| match e {
        accounts::AccountError::Database(e) => ActionError::Database(e),
        other => ActionError::Invalid(other.to_string()),
    })?;
    Ok(Output::new(json!({
        "created": { "path": account.full_path, "name": account.name, "type": account.kind },
    }))
    .with_change(
        format!(
            "Created category \"{}\" under {}",
            account.name, parent.full_path
        ),
        CreatedCategory {
            account_id: account.id,
        },
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenameCategoryInput {
    /// The category to rename (path or name).
    category: String,
    /// Its new display name.
    #[schemars(length(min = 1, max = 60))]
    new_name: String,
}

impl Check for RenameCategoryInput {
    fn check(&self) -> Result<(), String> {
        length("new_name", &self.new_name, 1, 60)
    }
}

#[derive(Serialize, Deserialize)]
pub struct Renamed {
    pub account_id: i64,
    pub from_path: String,
    pub from_name: String,
    pub to_path: String,
}

pub async fn rename_category(
    db: &SqlitePool,
    input: RenameCategoryInput,
) -> Result<Output, ActionError> {
    let account = accounts::find_category(db, &input.category).await?;
    let Some((parent_path, _)) = account.full_path.rsplit_once(':') else {
        return Err(cannot_rename(&account.full_path));
    };
    if account.full_path == UNCATEGORISED_PATH {
        return Err(cannot_rename(&account.full_path));
    }
    let segment = path_segment(&input.new_name);
    if segment.is_empty() {
        return Err(ActionError::invalid(
            "The name needs at least one letter or number.",
        ));
    }
    let new_path = format!("{parent_path}:{segment}");
    if new_path != account.full_path && path_taken(db, &new_path).await? {
        return Err(ActionError::Invalid(format!(
            "A category {} already exists.",
            quoted(&new_path)
        )));
    }
    let new_name = input.new_name.trim();
    let mut tx = db.begin().await?;
    let moved = move_account(&mut tx, account.id, &account.full_path, &new_path, new_name).await?;
    tx.commit().await?;
    Ok(Output::new(json!({
        "renamed": { "from": account.full_path, "to": new_path, "subcategories_moved": moved },
    }))
    .with_change(
        format!("Renamed category \"{}\" to \"{new_name}\"", account.name),
        Renamed {
            account_id: account.id,
            from_path: account.full_path.clone(),
            from_name: account.name.clone(),
            to_path: new_path,
        },
    ))
}

fn cannot_rename(path: &str) -> ActionError {
    ActionError::Invalid(format!(
        "{} can't be renamed; the app relies on it.",
        quoted(path)
    ))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum RuleMatch {
    #[default]
    Substring,
    Prefix,
    Regex,
}

impl From<RuleMatch> for MatchType {
    fn from(kind: RuleMatch) -> Self {
        match kind {
            RuleMatch::Substring => Self::Substring,
            RuleMatch::Prefix => Self::Prefix,
            RuleMatch::Regex => Self::Regex,
        }
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateRuleInput {
    /// Text to match in descriptions.
    #[schemars(length(min = 1, max = 200))]
    pattern: String,
    /// Category to assign (path or name).
    category: String,
    /// "substring" matches anywhere, "prefix" only at the start, "regex" is a
    /// case-insensitive regular expression.
    #[serde(default)]
    match_type: RuleMatch,
    /// Higher-priority rules win.
    #[serde(default = "ten")]
    #[schemars(range(min = 1, max = 100))]
    priority: i64,
    /// A short note on what the rule matches.
    #[serde(default)]
    #[schemars(length(max = 200))]
    description: Option<String>,
}

fn ten() -> i64 {
    10
}

impl Check for CreateRuleInput {
    fn check(&self) -> Result<(), String> {
        length("pattern", &self.pattern, 1, 200)?;
        in_range("priority", self.priority, 1, 100)?;
        if let Some(description) = &self.description {
            length("description", description, 0, 200)?;
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
pub struct CreatedRule {
    pub rule_id: i64,
}

pub async fn create_rule(db: &SqlitePool, input: CreateRuleInput) -> Result<Output, ActionError> {
    let category = accounts::find_category(db, &input.category).await?;
    let match_type = MatchType::from(input.match_type);
    if let Err(rules::RuleError::BadRegex(reason)) =
        rules::check_pattern(&input.pattern, match_type)
    {
        return Err(ActionError::Invalid(format!(
            "That regular expression is invalid: {reason}"
        )));
    }
    let duplicate: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM rules
         WHERE pattern = ? AND match_type_id = ? AND target_account_id = ?)",
    )
    .bind(&input.pattern)
    .bind(match_type as i64)
    .bind(category.id)
    .fetch_one(db)
    .await?;
    if duplicate {
        return Err(ActionError::invalid("That rule already exists."));
    }
    let rule = rules::create(
        db,
        NewRule {
            pattern: input.pattern,
            match_type_id: match_type as i64,
            target_account_id: category.id,
            priority: input.priority,
            source: "ai".into(),
            description: input.description,
        },
    )
    .await
    .map_err(|e| match e {
        rules::RuleError::Database(e) => ActionError::Database(e),
        other => ActionError::Invalid(other.to_string()),
    })?;
    Ok(Output::new(json!({
        "created": {
            "id": rule.id,
            "pattern": rule.pattern,
            "match_type": match_type.name(),
            "category": category.full_path,
            "priority": rule.priority,
        },
    }))
    .with_change(
        format!("Created rule \"{}\" → {}", rule.pattern, category.full_path),
        CreatedRule { rule_id: rule.id },
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecategoriseInput {
    /// IDs from list_transactions.
    #[schemars(length(min = 1, max = 200))]
    transaction_ids: Vec<i64>,
    /// The category to move them to (path or name).
    category: String,
}

impl Check for RecategoriseInput {
    fn check(&self) -> Result<(), String> {
        items("transaction_ids", &self.transaction_ids, 1, 200)
    }
}

/// A posting's category as it was, for undoing a move.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PostingBefore {
    pub id: i64,
    pub account_id: i64,
    pub categorised_by_id: i64,
    pub rule_id: Option<i64>,
}

#[derive(Serialize, Deserialize)]
pub struct Recategorised {
    pub postings: Vec<PostingBefore>,
}

pub async fn recategorise_transactions(
    db: &SqlitePool,
    input: RecategoriseInput,
) -> Result<Output, ActionError> {
    let category = accounts::find_category(db, &input.category).await?;
    let ids = unique(&input.transaction_ids);
    let loaded = common::load(db, &ids).await?;
    let (mut updated, mut skipped, mut before) = (Vec::new(), Vec::new(), Vec::new());
    for id in ids {
        let Some(txn) = loaded.get(&id) else {
            skipped.push(json!({ "id": id, "reason": "not found" }));
            continue;
        };
        match txn.categories().as_slice() {
            [] => skipped.push(json!({ "id": id, "reason": "has no category (e.g. a transfer)" })),
            [posting] => {
                before.push(PostingBefore {
                    id: posting.id,
                    account_id: posting.account_id,
                    categorised_by_id: posting.categorised_by_id,
                    rule_id: posting.rule_id,
                });
                updated.push(id);
            }
            _ => skipped.push(json!({ "id": id, "reason": "split across several categories" })),
        }
    }
    let mut tx = db.begin().await?;
    for posting in &before {
        sqlx::query(
            "UPDATE postings SET account_id = ?, categorised_by_id = ?, rule_id = NULL
             WHERE id = ?",
        )
        .bind(category.id)
        .bind(CategorisationSource::Ai as i64)
        .bind(posting.id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    let output = Output::new(json!({
        "category": category.full_path,
        "updated": updated,
        "skipped": skipped,
    }));
    if updated.is_empty() {
        return Ok(output);
    }
    Ok(output.with_change(
        format!(
            "Moved {} to {}",
            count_of(updated.len(), "transaction"),
            category.full_path
        ),
        Recategorised { postings: before },
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetBudgetInput {
    /// Spending category (path or name); includes subcategories.
    category: String,
    /// The spending limit for the period.
    amount: Money,
    /// Whether the budget is monthly or annual.
    #[serde(default)]
    period: Period,
}

impl Check for SetBudgetInput {
    fn check(&self) -> Result<(), String> {
        if self.amount.is_positive() {
            Ok(())
        } else {
            Err("amount: must be greater than 0".into())
        }
    }
}

/// A budget's limit before `set_budget` changed it, if it had one. Amounts are strings with
/// two decimal places.
#[derive(Serialize, Deserialize)]
pub struct BudgetSet {
    pub account_id: i64,
    pub period_id: i64,
    pub previous: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct BudgetDeleted {
    pub account_id: i64,
    pub period_id: i64,
    pub amount: String,
}

fn budget_error(e: BudgetError) -> ActionError {
    match e {
        BudgetError::Database(e) => ActionError::Database(e),
        BudgetError::NotSpending => {
            ActionError::invalid("Budgets can only be set on spending categories.")
        }
        other => ActionError::Invalid(other.to_string()),
    }
}

pub async fn set_budget(db: &SqlitePool, input: SetBudgetInput) -> Result<Output, ActionError> {
    let category = accounts::find_category(db, &input.category).await?;
    if category.kind != AccountType::Expense {
        return Err(budget_error(BudgetError::NotSpending));
    }
    let previous = budgets::set(db, category.id, input.period, input.amount)
        .await
        .map_err(budget_error)?
        .map(Money::two_places);
    let limit = input.amount.two_places();
    let verb = if previous.is_some() {
        format!("Changed the {} budget for", input.period.name())
    } else {
        format!("Set a {} budget for", input.period.name())
    };
    Ok(Output::new(json!({
        "budget": { "category": category.full_path, "period": input.period, "limit": limit },
        "previous_limit": previous,
    }))
    .with_change(
        format!("{verb} {} to £{limit}", category.full_path),
        BudgetSet {
            account_id: category.id,
            period_id: input.period.id(),
            previous,
        },
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteBudgetInput {
    /// Spending category (path or name).
    category: String,
    /// Whether the budget is monthly or annual.
    #[serde(default)]
    period: Period,
}

impl Check for DeleteBudgetInput {}

pub async fn delete_budget(
    db: &SqlitePool,
    input: DeleteBudgetInput,
) -> Result<Output, ActionError> {
    let category = accounts::find_category(db, &input.category).await?;
    let Some(removed) = budgets::remove(db, category.id, input.period).await? else {
        return Err(ActionError::Invalid(format!(
            "{} has no {} budget.",
            quoted(&category.full_path),
            input.period.name()
        )));
    };
    let limit = removed.two_places();
    Ok(Output::new(json!({
        "deleted": { "category": category.full_path, "period": input.period, "limit": limit },
    }))
    .with_change(
        format!(
            "Removed the {} budget for {}",
            input.period.name(),
            category.full_path
        ),
        BudgetDeleted {
            account_id: category.id,
            period_id: input.period.id(),
            amount: limit,
        },
    ))
}

/// A two-place amount from undo data, as budgets store it.
pub fn stored_amount(text: &str) -> Option<Money> {
    text.parse::<Decimal>()
        .ok()
        .and_then(|d| Money::from_decimal(d).ok())
}
