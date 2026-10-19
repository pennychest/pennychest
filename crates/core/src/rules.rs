//! Categorisation rules: a pattern matched against a transaction's description, and the
//! category it files matching transactions under. The highest priority rule that matches wins,
//! and among equal priorities the oldest.

use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Deserializer, Serialize};
use sqlx::{SqliteConnection, SqlitePool};

use crate::lookups::{CategorisationSource, MatchType};

/// Big enough for any pattern a person writes, small enough that a pathological one can't
/// tie up the server.
const REGEX_SIZE_LIMIT: usize = 1 << 20;
const PREVIEW_TRANSACTIONS: i64 = 500;
const APPLY_PREVIEW_TRANSACTIONS: i64 = 1000;

/// A pattern ready to test descriptions against. Case never matters.
#[derive(Debug, Clone)]
pub enum Matcher {
    Substring(String),
    Prefix(String),
    Regex(Regex),
    /// A regular expression that doesn't compile. It matches nothing;
    /// new rules can't be saved with one.
    Never,
}

impl Matcher {
    pub fn new(pattern: &str, match_type: MatchType) -> Self {
        match match_type {
            MatchType::Substring => Self::Substring(pattern.to_uppercase()),
            MatchType::Prefix => Self::Prefix(pattern.to_uppercase()),
            MatchType::Regex => compile(pattern).map_or(Self::Never, Self::Regex),
        }
    }

    pub fn matches(&self, description: &str) -> bool {
        match self {
            Self::Substring(pattern) => description.to_uppercase().contains(pattern.as_str()),
            Self::Prefix(pattern) => description.to_uppercase().starts_with(pattern.as_str()),
            Self::Regex(regex) => regex.is_match(description),
            Self::Never => false,
        }
    }
}

fn compile(pattern: &str) -> Result<Regex, regex::Error> {
    RegexBuilder::new(pattern)
        .case_insensitive(true)
        .size_limit(REGEX_SIZE_LIMIT)
        .build()
}

pub fn matches_rule(description: &str, pattern: &str, match_type: MatchType) -> bool {
    Matcher::new(pattern, match_type).matches(description)
}

/// Whether a pattern can be saved: regular expressions must compile, and the `regex` crate
/// doesn't support lookaround or backreferences.
pub fn check_pattern(pattern: &str, match_type: MatchType) -> Result<(), RuleError> {
    if match_type == MatchType::Regex
        && let Err(err) = compile(pattern)
    {
        let reason = err.to_string();
        let reason = reason.lines().last().unwrap_or(&reason).trim().to_owned();
        return Err(RuleError::BadRegex(reason));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize, sqlx::FromRow)]
pub struct Rule {
    pub id: i64,
    pub pattern: String,
    pub match_type_id: i64,
    pub match_type_name: Option<String>,
    pub target_account_id: i64,
    pub target_account_path: Option<String>,
    pub priority: i64,
    pub source: Option<String>,
    pub description: Option<String>,
}

impl Rule {
    /// How the pattern is matched. An unknown match type is treated as a substring.
    pub fn match_type(&self) -> MatchType {
        MatchType::try_from(self.match_type_id).unwrap_or(MatchType::Substring)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct NewRule {
    pub pattern: String,
    pub match_type_id: i64,
    pub target_account_id: i64,
    #[serde(default)]
    pub priority: i64,
    #[serde(default = "manual")]
    pub source: String,
    #[serde(default)]
    pub description: Option<String>,
}

fn manual() -> String {
    "manual".into()
}

/// Changes to a rule. Fields left out stay as they are; the description can be cleared with
/// `null`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RuleUpdate {
    pub pattern: Option<String>,
    pub match_type_id: Option<i64>,
    pub target_account_id: Option<i64>,
    pub priority: Option<i64>,
    #[serde(default, deserialize_with = "present")]
    pub description: Option<Option<String>>,
}

fn present<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// A rule as rules are exported and imported: by names, not ids.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportedRule {
    #[serde(default)]
    pub pattern: String,
    #[serde(default = "substring")]
    pub match_type: String,
    #[serde(default)]
    pub target_account: String,
    #[serde(default)]
    pub priority: i64,
}

fn substring() -> String {
    "substring".into()
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Conflict {
    pub rule_id: i64,
    pub pattern: String,
    pub match_type: &'static str,
    pub priority: i64,
    pub target_account_id: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, sqlx::FromRow)]
pub struct MatchingTransaction {
    pub id: i64,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Preview {
    pub matching_transactions: Vec<MatchingTransaction>,
    pub match_count: usize,
    pub conflicts: Vec<Conflict>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TestResult {
    pub matching: Vec<String>,
    pub match_count: usize,
    pub total: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ApplyPreviewItem {
    pub transaction_id: i64,
    pub description: String,
    pub current_account_path: Option<String>,
    pub new_account_path: Option<String>,
    pub rule_id: i64,
    pub rule_pattern: String,
    /// Categorised by hand, so re-applying rules leaves it alone.
    pub is_manual: bool,
    pub would_change: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ApplyPreview {
    pub items: Vec<ApplyPreviewItem>,
    pub would_change_count: usize,
    pub manual_conflict_count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Imported {
    pub imported: usize,
    pub skipped: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum RuleError {
    #[error("Rule not found")]
    NotFound,
    #[error("Match type {0} not found")]
    MatchTypeNotFound(i64),
    #[error("Account {0} not found")]
    AccountNotFound(i64),
    #[error("That pattern isn't a regular expression PennyChest can use: {0}")]
    BadRegex(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

const SELECT_RULES: &str = "SELECT r.id, r.pattern, r.match_type_id, m.name AS match_type_name,
            r.target_account_id, a.full_path AS target_account_path, r.priority, r.source,
            r.description
     FROM rules r
     LEFT JOIN match_types m ON m.id = r.match_type_id
     LEFT JOIN accounts a ON a.id = r.target_account_id";

/// Every rule, in the order they're tried: highest priority first, then oldest.
pub async fn list(db: &mut SqliteConnection) -> Result<Vec<Rule>, sqlx::Error> {
    sqlx::query_as(&format!("{SELECT_RULES} ORDER BY r.priority DESC, r.id"))
        .fetch_all(db)
        .await
}

pub async fn get(db: &mut SqliteConnection, id: i64) -> Result<Option<Rule>, sqlx::Error> {
    sqlx::query_as(&format!("{SELECT_RULES} WHERE r.id = ?"))
        .bind(id)
        .fetch_optional(db)
        .await
}

async fn check(
    db: &mut SqliteConnection,
    pattern: &str,
    match_type_id: i64,
    target_account_id: i64,
) -> Result<(), RuleError> {
    let match_type = MatchType::try_from(match_type_id).map_err(RuleError::MatchTypeNotFound)?;
    check_pattern(pattern, match_type)?;
    let account: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM accounts WHERE id = ?)")
        .bind(target_account_id)
        .fetch_one(&mut *db)
        .await?;
    if !account {
        return Err(RuleError::AccountNotFound(target_account_id));
    }
    Ok(())
}

pub async fn create(db: &SqlitePool, new: NewRule) -> Result<Rule, RuleError> {
    let mut tx = db.begin().await?;
    check(
        &mut tx,
        &new.pattern,
        new.match_type_id,
        new.target_account_id,
    )
    .await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO rules (pattern, match_type_id, target_account_id, priority, source, description)
         VALUES (?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(&new.pattern)
    .bind(new.match_type_id)
    .bind(new.target_account_id)
    .bind(new.priority)
    .bind(&new.source)
    .bind(&new.description)
    .fetch_one(&mut *tx)
    .await?;
    let rule = get(&mut tx, id).await?.ok_or(RuleError::NotFound)?;
    tx.commit().await?;
    Ok(rule)
}

pub async fn update(db: &SqlitePool, id: i64, changes: RuleUpdate) -> Result<Rule, RuleError> {
    let mut tx = db.begin().await?;
    let mut rule = get(&mut tx, id).await?.ok_or(RuleError::NotFound)?;
    if let Some(pattern) = changes.pattern {
        rule.pattern = pattern;
    }
    if let Some(match_type_id) = changes.match_type_id {
        rule.match_type_id = match_type_id;
    }
    if let Some(target_account_id) = changes.target_account_id {
        rule.target_account_id = target_account_id;
    }
    if let Some(priority) = changes.priority {
        rule.priority = priority;
    }
    if let Some(description) = changes.description {
        rule.description = description;
    }
    check(
        &mut tx,
        &rule.pattern,
        rule.match_type_id,
        rule.target_account_id,
    )
    .await?;
    sqlx::query(
        "UPDATE rules SET pattern = ?, match_type_id = ?, target_account_id = ?, priority = ?,
         description = ? WHERE id = ?",
    )
    .bind(&rule.pattern)
    .bind(rule.match_type_id)
    .bind(rule.target_account_id)
    .bind(rule.priority)
    .bind(&rule.description)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    let rule = get(&mut tx, id).await?.ok_or(RuleError::NotFound)?;
    tx.commit().await?;
    Ok(rule)
}

/// Deletes a rule. Postings it categorised keep their category but stop pointing at it.
pub async fn delete(db: &SqlitePool, id: i64) -> Result<(), RuleError> {
    let result = sqlx::query("DELETE FROM rules WHERE id = ?")
        .bind(id)
        .execute(db)
        .await?;
    if result.rows_affected() == 0 {
        return Err(RuleError::NotFound);
    }
    Ok(())
}

/// Every rule, compiled once, in the order they're tried.
pub struct RuleSet {
    rules: Vec<(Rule, Matcher)>,
}

impl RuleSet {
    pub async fn load(db: &mut SqliteConnection) -> Result<Self, sqlx::Error> {
        Ok(Self::new(list(db).await?))
    }

    /// From rules already in the order they're tried.
    pub fn new(rules: Vec<Rule>) -> Self {
        Self {
            rules: rules
                .into_iter()
                .map(|rule| {
                    let matcher = Matcher::new(&rule.pattern, rule.match_type());
                    (rule, matcher)
                })
                .collect(),
        }
    }

    /// The rule that categorises this description, if any.
    pub fn first_match(&self, description: &str) -> Option<&Rule> {
        self.rules
            .iter()
            .find(|(_, matcher)| matcher.matches(description))
            .map(|(rule, _)| rule)
    }

    /// Rules that could match the same descriptions as this pattern. A heuristic: each
    /// pattern is tried against the other as though it were a description.
    pub fn conflicts(
        &self,
        pattern: &str,
        match_type: MatchType,
        exclude: Option<i64>,
    ) -> Vec<Conflict> {
        let new = Matcher::new(pattern, match_type);
        self.rules
            .iter()
            .filter(|(rule, _)| exclude != Some(rule.id))
            .filter(|(rule, existing)| new.matches(&rule.pattern) || existing.matches(pattern))
            .map(|(rule, _)| Conflict {
                rule_id: rule.id,
                pattern: rule.pattern.clone(),
                match_type: rule.match_type().name(),
                priority: rule.priority,
                target_account_id: rule.target_account_id,
            })
            .collect()
    }
}

/// Which recent transactions a pattern would match, and which rules it would overlap with.
pub async fn preview(
    db: &SqlitePool,
    pattern: &str,
    match_type_id: i64,
    exclude_rule_id: Option<i64>,
) -> Result<Preview, sqlx::Error> {
    let match_type = MatchType::try_from(match_type_id).unwrap_or(MatchType::Substring);
    let matcher = Matcher::new(pattern, match_type);
    let mut conn = db.acquire().await?;
    let recent: Vec<MatchingTransaction> = sqlx::query_as(
        "SELECT id, description FROM transactions ORDER BY date DESC, id DESC LIMIT ?",
    )
    .bind(PREVIEW_TRANSACTIONS)
    .fetch_all(&mut *conn)
    .await?;
    let matching: Vec<MatchingTransaction> = recent
        .into_iter()
        .filter(|t| matcher.matches(&t.description))
        .collect();
    let conflicts = RuleSet::load(&mut conn)
        .await?
        .conflicts(pattern, match_type, exclude_rule_id);
    Ok(Preview {
        match_count: matching.len(),
        matching_transactions: matching,
        conflicts,
    })
}

/// Which of `descriptions` a pattern would match.
pub fn test(pattern: &str, match_type_id: i64, descriptions: Vec<String>) -> TestResult {
    let match_type = MatchType::try_from(match_type_id).unwrap_or(MatchType::Substring);
    let matcher = Matcher::new(pattern, match_type);
    let total = descriptions.len();
    let matching: Vec<String> = descriptions
        .into_iter()
        .filter(|d| matcher.matches(d))
        .collect();
    TestResult {
        match_count: matching.len(),
        matching,
        total,
    }
}

/// What re-applying every rule to recent transactions would change: each category posting
/// (on an income or expense account) that the matching rule would move. Ones categorised by
/// hand are listed but marked, since re-applying leaves them alone.
pub async fn apply_preview(db: &SqlitePool) -> Result<ApplyPreview, sqlx::Error> {
    let mut conn = db.acquire().await?;
    let rules = RuleSet::load(&mut conn).await?;
    let recent: Vec<MatchingTransaction> = sqlx::query_as(
        "SELECT id, description FROM transactions ORDER BY date DESC, id DESC LIMIT ?",
    )
    .bind(APPLY_PREVIEW_TRANSACTIONS)
    .fetch_all(&mut *conn)
    .await?;

    let mut items = Vec::new();
    for transaction in recent {
        let Some(rule) = rules.first_match(&transaction.description) else {
            continue;
        };
        let categories: Vec<(i64, i64, String)> = sqlx::query_as(
            "SELECT p.account_id, p.categorised_by_id, a.full_path
             FROM postings p JOIN accounts a ON a.id = p.account_id
             WHERE p.transaction_id = ? AND a.type IN ('income', 'expense')
             ORDER BY p.id",
        )
        .bind(transaction.id)
        .fetch_all(&mut *conn)
        .await?;
        for (account_id, source, path) in categories {
            if account_id == rule.target_account_id {
                continue;
            }
            items.push(ApplyPreviewItem {
                transaction_id: transaction.id,
                description: transaction.description.clone(),
                current_account_path: Some(path),
                new_account_path: rule.target_account_path.clone(),
                rule_id: rule.id,
                rule_pattern: rule.pattern.clone(),
                is_manual: source == CategorisationSource::Manual as i64,
                would_change: true,
            });
        }
    }
    Ok(ApplyPreview {
        would_change_count: items
            .iter()
            .filter(|i| i.would_change && !i.is_manual)
            .count(),
        manual_conflict_count: items.iter().filter(|i| i.is_manual).count(),
        items,
    })
}

/// Every rule by names, in the order they're tried.
pub async fn export(db: &SqlitePool) -> Result<Vec<ExportedRule>, sqlx::Error> {
    let rules = list(&mut *db.acquire().await?).await?;
    Ok(rules
        .into_iter()
        .map(|rule| ExportedRule {
            match_type: rule.match_type().name().to_owned(),
            target_account: rule.target_account_path.unwrap_or_default(),
            pattern: rule.pattern,
            priority: rule.priority,
        })
        .collect())
}

/// Adds rules, skipping ones whose match type or account doesn't exist, whose pattern is
/// empty or isn't a usable regular expression, or that exist already.
pub async fn import(db: &SqlitePool, rules: Vec<ExportedRule>) -> Result<Imported, sqlx::Error> {
    let mut tx = db.begin().await?;
    let mut imported = 0;
    let mut skipped = 0;
    for rule in rules {
        let match_type = MatchType::from_name(&rule.match_type);
        let target: Option<i64> = sqlx::query_scalar("SELECT id FROM accounts WHERE full_path = ?")
            .bind(&rule.target_account)
            .fetch_optional(&mut *tx)
            .await?;
        let (Some(match_type), Some(target)) = (match_type, target) else {
            skipped += 1;
            continue;
        };
        if rule.pattern.is_empty() || check_pattern(&rule.pattern, match_type).is_err() {
            skipped += 1;
            continue;
        }
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM rules
                            WHERE pattern = ? AND match_type_id = ? AND target_account_id = ?)",
        )
        .bind(&rule.pattern)
        .bind(match_type as i64)
        .bind(target)
        .fetch_one(&mut *tx)
        .await?;
        if exists {
            skipped += 1;
            continue;
        }
        sqlx::query(
            "INSERT INTO rules (pattern, match_type_id, target_account_id, priority, source)
             VALUES (?, ?, ?, ?, 'import')",
        )
        .bind(&rule.pattern)
        .bind(match_type as i64)
        .bind(target)
        .bind(rule.priority)
        .execute(&mut *tx)
        .await?;
        imported += 1;
    }
    tx.commit().await?;
    Ok(Imported { imported, skipped })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts;
    use crate::db;

    #[test]
    fn matches_ignoring_case() {
        assert!(matches_rule(
            "Tesco Stores 2041",
            "TESCO",
            MatchType::Substring
        ));
        assert!(matches_rule("TESCO STORES", "stores", MatchType::Substring));
        assert!(matches_rule("TESCO STORES", "tesco", MatchType::Prefix));
        assert!(!matches_rule("MY TESCO", "tesco", MatchType::Prefix));
        assert!(matches_rule(
            "Card payment to TfL",
            r"tfl|oyster",
            MatchType::Regex
        ));
        assert!(matches_rule(
            "AMAZON.CO.UK*2X4",
            r"^amazon\.co\.uk",
            MatchType::Regex
        ));
        assert!(!matches_rule(
            "AMAZONIAN",
            r"^amazon\.co\.uk",
            MatchType::Regex
        ));
        // An empty substring matches everything.
        assert!(matches_rule("anything", "", MatchType::Substring));
    }

    #[test]
    fn unusable_regexes_never_match_and_cant_be_saved() {
        assert!(!matches_rule("abc", "(", MatchType::Regex));
        assert!(matches!(
            check_pattern("(", MatchType::Regex),
            Err(RuleError::BadRegex(_))
        ));
        // Lookaround isn't supported.
        assert!(check_pattern("foo(?!bar)", MatchType::Regex).is_err());
        assert!(check_pattern("(", MatchType::Substring).is_ok());
    }

    fn rule(id: i64, pattern: &str, match_type: MatchType, priority: i64) -> Rule {
        Rule {
            id,
            pattern: pattern.into(),
            match_type_id: match_type as i64,
            match_type_name: Some(match_type.name().into()),
            target_account_id: id * 10,
            target_account_path: None,
            priority,
            source: None,
            description: None,
        }
    }

    #[test]
    fn the_highest_priority_then_oldest_rule_wins() {
        let rules = RuleSet::new(vec![
            rule(3, "TESCO EXPRESS", MatchType::Substring, 20),
            rule(1, "TESCO", MatchType::Substring, 10),
            rule(2, "TES", MatchType::Prefix, 10),
        ]);
        assert_eq!(rules.first_match("TESCO EXPRESS 123").unwrap().id, 3);
        assert_eq!(rules.first_match("TESCO STORES").unwrap().id, 1);
        assert_eq!(rules.first_match("TESLA").unwrap().id, 2);
        assert!(rules.first_match("ASDA").is_none());
    }

    #[test]
    fn finds_overlapping_rules() {
        let rules = RuleSet::new(vec![
            rule(1, "TESCO", MatchType::Substring, 10),
            rule(2, "ASDA", MatchType::Substring, 10),
            rule(3, "TESCO EXPRESS", MatchType::Substring, 0),
        ]);
        let ids = |c: Vec<Conflict>| c.into_iter().map(|c| c.rule_id).collect::<Vec<_>>();
        assert_eq!(
            ids(rules.conflicts("TESCO", MatchType::Substring, None)),
            [1, 3]
        );
        assert_eq!(
            ids(rules.conflicts("TESCO", MatchType::Substring, Some(1))),
            [3]
        );
        assert!(
            rules
                .conflicts("LIDL", MatchType::Substring, None)
                .is_empty()
        );
    }

    async fn seeded() -> SqlitePool {
        let pool = db::open_in_memory().await.unwrap();
        accounts::load_seed_accounts(&pool).await.unwrap();
        pool
    }

    async fn account(pool: &SqlitePool, path: &str) -> i64 {
        accounts::find_by_path(&mut pool.acquire().await.unwrap(), path)
            .await
            .unwrap()
            .unwrap()
            .id
    }

    #[tokio::test]
    async fn creates_updates_and_deletes() {
        let pool = seeded().await;
        let groceries = account(&pool, "Expenses:Food:Groceries").await;
        let created = create(
            &pool,
            NewRule {
                pattern: "TESCO".into(),
                match_type_id: 1,
                target_account_id: groceries,
                priority: 10,
                source: "manual".into(),
                description: Some("Supermarket".into()),
            },
        )
        .await
        .unwrap();
        assert_eq!(created.match_type_name.as_deref(), Some("substring"));
        assert_eq!(
            created.target_account_path.as_deref(),
            Some("Expenses:Food:Groceries")
        );

        let updated = update(
            &pool,
            created.id,
            RuleUpdate {
                pattern: Some("^TESCO".into()),
                match_type_id: Some(3),
                description: Some(None),
                ..RuleUpdate::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(updated.match_type_name.as_deref(), Some("regex"));
        assert_eq!(updated.description, None);
        assert_eq!(updated.priority, 10);

        let bad = update(
            &pool,
            created.id,
            RuleUpdate {
                pattern: Some("(".into()),
                ..RuleUpdate::default()
            },
        )
        .await;
        assert!(matches!(bad, Err(RuleError::BadRegex(_))));

        delete(&pool, created.id).await.unwrap();
        assert!(matches!(
            delete(&pool, created.id).await,
            Err(RuleError::NotFound)
        ));
    }

    #[tokio::test]
    async fn refuses_unknown_match_types_and_accounts() {
        let pool = seeded().await;
        let groceries = account(&pool, "Expenses:Food:Groceries").await;
        let new = |match_type_id, target_account_id| NewRule {
            pattern: "X".into(),
            match_type_id,
            target_account_id,
            priority: 0,
            source: "manual".into(),
            description: None,
        };
        assert!(matches!(
            create(&pool, new(9, groceries)).await,
            Err(RuleError::MatchTypeNotFound(9))
        ));
        assert!(matches!(
            create(&pool, new(1, 999)).await,
            Err(RuleError::AccountNotFound(999))
        ));
    }

    #[tokio::test]
    async fn exports_and_imports_by_name() {
        let pool = seeded().await;
        accounts::load_seed_rules(&pool).await.unwrap();
        let exported = export(&pool).await.unwrap();
        assert_eq!(exported.len(), 20);
        assert_eq!(
            import(&pool, exported.clone()).await.unwrap(),
            Imported {
                imported: 0,
                skipped: 20
            }
        );

        let fresh = seeded().await;
        let mut with_bad = exported.clone();
        with_bad.push(ExportedRule {
            pattern: "X".into(),
            match_type: "fuzzy".into(),
            target_account: "Expenses:Food".into(),
            priority: 0,
        });
        with_bad.push(ExportedRule {
            pattern: "(".into(),
            match_type: "regex".into(),
            target_account: "Expenses:Food".into(),
            priority: 0,
        });
        assert_eq!(
            import(&fresh, with_bad).await.unwrap(),
            Imported {
                imported: 20,
                skipped: 2
            }
        );
        let sources: Vec<String> = sqlx::query_scalar("SELECT DISTINCT source FROM rules")
            .fetch_all(&fresh)
            .await
            .unwrap();
        assert_eq!(sources, ["import"]);
    }
}
