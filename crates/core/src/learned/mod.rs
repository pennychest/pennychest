//! Categorising from the user's own history: a model trained on the transactions they've
//! categorised, used before any outside AI.
//!
//! It learns only from categories the user stands behind: set by hand, by one of their rules,
//! or confirmed. The model is retrained whenever that data changes, which takes about a
//! second, so there's nothing to store or migrate.

pub mod model;

use std::sync::{Arc, Mutex};

use rand::SeedableRng;
use rand::seq::SliceRandom;
use serde::Serialize;
use sqlx::SqlitePool;

use self::model::{Classifier, Vectoriser};
use crate::imports::batches::UNCATEGORISED_PATH;
use crate::settings::{Setting, SettingError};

pub const ENABLED: Setting<String> = Setting::new("ai.learned.enabled");
/// Below this it leaves the line for the AI (or the user).
pub const CONFIDENCE: f64 = 0.7;
pub const MIN_EXAMPLES: usize = 30;
/// The regularisation: higher fits the history more closely.
const STRENGTH: f64 = 20.0;
const CHECK_FOLDS: usize = 5;
/// Payment processors that prefix the real merchant's name.
const PREFIXES: &[&str] = &[
    "SQ", "SUMUP", "ZETTLE", "IZ", "CRV", "PAYPAL", "PP", "CKO", "SP", "WWW",
];

/// A description reduced to the words that name the merchant: upper case, digits and
/// punctuation gone, payment-processor prefixes dropped.
pub fn clean(text: &str) -> String {
    let upper = text.to_uppercase();
    let letters: String = upper
        .chars()
        .map(|c| {
            if c.is_ascii_uppercase() || c == '&' {
                c
            } else {
                ' '
            }
        })
        .collect();
    letters
        .split_whitespace()
        .filter(|word| !PREFIXES.contains(word))
        .collect::<Vec<_>>()
        .join(" ")
}

pub async fn enabled(db: &SqlitePool) -> Result<bool, SettingError> {
    Ok(ENABLED.get(db).await?.as_deref() != Some("off"))
}

pub async fn set_enabled(db: &SqlitePool, on: bool) -> Result<(), SettingError> {
    ENABLED
        .set(db, &if on { "on" } else { "off" }.to_owned())
        .await
}

/// Category postings the user stands behind, on transactions with exactly one category:
/// (posting id, category account id, description).
const TRUSTED: &str = "
    SELECT p.id, p.account_id, t.description
    FROM postings p
    JOIN transactions t ON t.id = p.transaction_id
    JOIN accounts a ON a.id = p.account_id
    WHERE a.type IN ('expense', 'income')
      AND a.full_path != ?
      AND (p.categorised_by_id IN (1, 2) OR t.status = 'confirmed')
      AND p.transaction_id IN (
          SELECT p2.transaction_id FROM postings p2
          JOIN accounts a2 ON a2.id = p2.account_id
          WHERE a2.type IN ('expense', 'income')
          GROUP BY p2.transaction_id HAVING COUNT(*) = 1)";

async fn trusted(db: &SqlitePool) -> Result<Vec<(i64, i64, String)>, sqlx::Error> {
    sqlx::query_as(&format!("{TRUSTED} ORDER BY p.id"))
        .bind(UNCATEGORISED_PATH)
        .fetch_all(db)
        .await
}

/// Changes whenever a trusted category is added, removed or changed.
type Signature = (i64, i64, i64);

async fn signature(db: &SqlitePool) -> Result<Signature, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT COUNT(*), coalesce(MAX(id), 0), coalesce(SUM(id * 7 + account_id), 0)
         FROM ({TRUSTED})"
    ))
    .bind(UNCATEGORISED_PATH)
    .fetch_one(db)
    .await
}

pub struct Model {
    vectoriser: Vectoriser,
    classifier: Classifier,
    pub examples: usize,
    pub categories: usize,
}

impl Model {
    /// A model of these (description, category) examples, or `None` until there are enough of
    /// them, in at least two categories.
    pub fn train(examples: &[(String, i64)]) -> Option<Self> {
        if examples.len() < MIN_EXAMPLES {
            return None;
        }
        Self::train_any(examples)
    }

    /// A model of however many examples there are, as long as they're in two categories or
    /// more. For evaluating; the app waits for `MIN_EXAMPLES`.
    pub fn train_any(examples: &[(String, i64)]) -> Option<Self> {
        let categories = distinct(examples.iter().map(|(_, c)| *c));
        if categories < 2 {
            return None;
        }
        let documents: Vec<String> = examples.iter().map(|(d, _)| clean(d)).collect();
        let labels: Vec<i64> = examples.iter().map(|(_, c)| *c).collect();
        let (vectoriser, rows) = Vectoriser::fit(&documents);
        let classifier = Classifier::fit(&rows, &labels, vectoriser.features(), STRENGTH);
        Some(Self {
            vectoriser,
            classifier,
            examples: examples.len(),
            categories,
        })
    }

    /// The likeliest category and its probability.
    pub fn predict(&self, description: &str) -> (i64, f64) {
        self.classifier
            .predict(&self.vectoriser.transform(&clean(description)))
    }
}

fn distinct(values: impl Iterator<Item = i64>) -> usize {
    let mut values: Vec<i64> = values.collect();
    values.sort_unstable();
    values.dedup();
    values.len()
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Check {
    /// The share of past transactions it would have categorised itself...
    pub coverage: f64,
    /// ...and how many of those it would have got right.
    pub accuracy: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Status {
    pub enabled: bool,
    pub examples: usize,
    pub categories: usize,
    pub min_examples: usize,
    pub confidence: f64,
    pub ready: bool,
    pub check: Option<Check>,
}

/// How the model would have done on the history it learns from: each fifth predicted by a
/// model trained on the other four.
pub fn cross_check(examples: &[(String, i64)]) -> Check {
    let mut order: Vec<usize> = (0..examples.len()).collect();
    order.shuffle(&mut rand::rngs::StdRng::seed_from_u64(0));
    let (mut sure, mut right) = (0_usize, 0_usize);
    let fold_size = examples.len().div_ceil(CHECK_FOLDS);
    for fold in order.chunks(fold_size.max(1)) {
        let train: Vec<(String, i64)> = order
            .iter()
            .filter(|i| !fold.contains(i))
            .map(|i| examples[*i].clone())
            .collect();
        if distinct(train.iter().map(|(_, c)| *c)) < 2 {
            continue;
        }
        let documents: Vec<String> = train.iter().map(|(d, _)| clean(d)).collect();
        let labels: Vec<i64> = train.iter().map(|(_, c)| *c).collect();
        let (vectoriser, rows) = Vectoriser::fit(&documents);
        let classifier = Classifier::fit(&rows, &labels, vectoriser.features(), STRENGTH);
        for i in fold {
            let (description, category) = &examples[*i];
            let (predicted, p) = classifier.predict(&vectoriser.transform(&clean(description)));
            if p >= CONFIDENCE {
                sure += 1;
                right += usize::from(predicted == *category);
            }
        }
    }
    Check {
        coverage: if examples.is_empty() {
            0.0
        } else {
            sure as f64 / examples.len() as f64
        },
        accuracy: (sure > 0).then(|| right as f64 / sure as f64),
    }
}

#[derive(Default)]
struct Cached {
    model: Option<(Signature, Option<Arc<Model>>)>,
    check: Option<(Signature, Check)>,
}

/// The trained model, kept until the history it learnt from changes. One per database, held
/// by the server.
#[derive(Default)]
pub struct Learned {
    cache: Mutex<Cached>,
}

impl Learned {
    fn cache(&self) -> std::sync::MutexGuard<'_, Cached> {
        self.cache.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The model for the current data, retrained if anything changed; `None` until there's
    /// enough history to learn from.
    pub async fn model(&self, db: &SqlitePool) -> Result<Option<Arc<Model>>, sqlx::Error> {
        let signature = signature(db).await?;
        if let Some((cached, model)) = &self.cache().model
            && *cached == signature
        {
            return Ok(model.clone());
        }
        let examples: Vec<(String, i64)> = trusted(db)
            .await?
            .into_iter()
            .map(|(_, account, description)| (description, account))
            .collect();
        let model = tokio::task::spawn_blocking(move || Model::train(&examples).map(Arc::new))
            .await
            .expect("training doesn't panic");
        self.cache().model = Some((signature, model.clone()));
        Ok(model)
    }

    /// For each description, (category account id, probability) when the model is confident
    /// enough to use, otherwise `None`.
    pub async fn suggest(
        &self,
        db: &SqlitePool,
        descriptions: &[String],
    ) -> Result<Vec<Option<(i64, f64)>>, SettingError> {
        if descriptions.is_empty() || !enabled(db).await? {
            return Ok(vec![None; descriptions.len()]);
        }
        let Some(model) = self.model(db).await? else {
            return Ok(vec![None; descriptions.len()]);
        };
        Ok(descriptions
            .iter()
            .map(|d| {
                let (category, p) = model.predict(d);
                (p >= CONFIDENCE).then_some((category, p))
            })
            .collect())
    }

    /// What the model has learned from, and how it does on that history.
    pub async fn status(&self, db: &SqlitePool) -> Result<Status, SettingError> {
        let examples: Vec<(String, i64)> = trusted(db)
            .await?
            .into_iter()
            .map(|(_, account, description)| (description, account))
            .collect();
        let categories = distinct(examples.iter().map(|(_, c)| *c));
        let ready = examples.len() >= MIN_EXAMPLES && categories >= 2;
        let check = if ready {
            let signature = signature(db).await?;
            let cached = self
                .cache()
                .check
                .filter(|(s, _)| *s == signature)
                .map(|(_, c)| c);
            match cached {
                Some(check) => Some(check),
                None => {
                    let history = examples.clone();
                    let check = tokio::task::spawn_blocking(move || cross_check(&history))
                        .await
                        .expect("checking doesn't panic");
                    self.cache().check = Some((signature, check));
                    Some(check)
                }
            }
        } else {
            None
        };
        Ok(Status {
            enabled: enabled(db).await?,
            examples: examples.len(),
            categories,
            min_examples: MIN_EXAMPLES,
            confidence: CONFIDENCE,
            ready,
            check,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_descriptions_down_to_the_merchant() {
        assert_eq!(clean("SQ *Corner Café 2041"), "CORNER CAF");
        assert_eq!(clean("PAYPAL *NETFLIX.COM"), "NETFLIX COM");
        assert_eq!(clean("M&S SIMPLY FOOD"), "M&S SIMPLY FOOD");
    }

    fn history() -> Vec<(String, i64)> {
        let mut history: Vec<(String, i64)> = (0..12)
            .map(|n| (format!("TESCO STORES {n} ST HELIER"), 1))
            .chain((0..12).map(|n| (format!("SOJ PARKING JERSEY {n}"), 2)))
            .collect();
        history.extend(std::iter::repeat_n(("NETFLIX.COM LONDON".to_owned(), 3), 8));
        history
    }

    #[test]
    fn needs_enough_history() {
        assert!(Model::train(&history()[..10]).is_none());
        let one_category: Vec<(String, i64)> = (0..40).map(|n| (format!("X {n}"), 1)).collect();
        assert!(Model::train(&one_category).is_none());
    }

    #[test]
    fn learns_merchants_and_is_unsure_of_strangers() {
        let model = Model::train(&history()).unwrap();
        let (category, p) = model.predict("TESCO STORES 4471 ST BRELADE");
        assert_eq!(category, 1);
        assert!(p >= CONFIDENCE, "{p}");
        let (_, p) = model.predict("PAYPAL PAYMENT");
        assert!(p < CONFIDENCE, "{p}");
    }

    #[test]
    fn checks_itself_on_its_history() {
        let check = cross_check(&history());
        assert!(check.coverage > 0.0 && check.coverage <= 1.0);
        assert!(check.accuracy.unwrap() > 0.9);
    }
}
