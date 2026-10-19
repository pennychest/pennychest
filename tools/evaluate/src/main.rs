//! How well the learned categoriser would do on your own data: would a model trained on what
//! you've already categorised get new transactions right? Nothing is changed; it only reads.
//!
//! ```sh
//! cargo run --release -p pennychest-evaluate -- --db pennychest.db
//! cargo run --release -p pennychest-evaluate -- --csv history.csv   # description, category[, date]
//! ```
//!
//! From a database, only transactions whose category you can trust are used; `--trust` picks
//! them from `manual` (categorised by hand), `rule` (by one of your rules), `confirmed` (any
//! you've confirmed) and `ai` (AI-categorised even if not confirmed, which measures agreement
//! with the AI rather than accuracy). The default is `manual,rule,confirmed`, what the app itself learns from.
//!
//! Two tests:
//!
//! - time split: train on the oldest 80%, test on the newest 20% (how it's used for real)
//! - cross-check: predict every transaction from the others; leave-one-out up to 300
//!   transactions, otherwise 10-fold

use std::collections::{BTreeMap, HashMap, HashSet};
use std::process::ExitCode;

use pennychest_core::learned::{CONFIDENCE, Model, clean};
use rand::SeedableRng;
use rand::seq::SliceRandom;
use sqlx::sqlite::SqliteConnectOptions;

const UNCATEGORISED: &str = "Expenses:Uncategorised";
const PREFIXES: &[&str] = &[
    "SQ", "SUMUP", "ZETTLE", "IZ", "CRV", "PAYPAL", "PP", "CKO", "SP", "WWW",
];

/// (date, description, category)
type Example = (String, String, String);
/// (transaction id, date, description, category, categorised by, status)
type CategoryRow = (i64, String, String, String, String, String);

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let trust: HashSet<String> = value("--trust")
        .unwrap_or_else(|| "manual,rule,confirmed".into())
        .split(',')
        .map(str::to_owned)
        .collect();
    let train_fraction: f64 = value("--train-fraction")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.8);
    let data = match (value("--db"), value("--csv")) {
        (Some(path), None) => load_db(&path, &trust).await,
        (None, Some(path)) => load_csv(&path),
        _ => Err("Give the data with --db PATH or --csv PATH.".into()),
    };
    let data = match data {
        Ok(data) => data,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(2);
        }
    };

    let mut categories: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, _, category) in &data {
        *categories.entry(category).or_default() += 1;
    }
    let mut most: Vec<(&&str, &usize)> = categories.iter().collect();
    most.sort_by(|a, b| b.1.cmp(a.1));
    let top: Vec<String> = most
        .iter()
        .take(5)
        .map(|(c, n)| format!("{c} ({n})"))
        .collect();
    println!(
        "{} categories; most used: {}",
        categories.len(),
        top.join(", ")
    );
    if data.len() < 20 || categories.len() < 2 {
        eprintln!("Too little data to evaluate.");
        return ExitCode::FAILURE;
    }

    time_split(&data, train_fraction);
    if !args.iter().any(|a| a == "--skip-cross-check") {
        let folds = value("--folds")
            .and_then(|v| v.parse().ok())
            .unwrap_or(if data.len() <= 300 { 0 } else { 10 });
        cross_check(&data, folds);
    }
    ExitCode::SUCCESS
}

async fn load_db(path: &str, trust: &HashSet<String>) -> Result<Vec<Example>, String> {
    let options = SqliteConnectOptions::new().filename(path).read_only(true);
    let db = sqlx::SqlitePool::connect_with(options)
        .await
        .map_err(|e| format!("Couldn't open {path}: {e}"))?;
    let rows: Vec<CategoryRow> = sqlx::query_as(
        "SELECT t.id, CAST(t.date AS TEXT), t.description, a.full_path, cs.name, t.status
         FROM transactions t
         JOIN postings p ON p.transaction_id = t.id
         JOIN accounts a ON a.id = p.account_id
         JOIN categorisation_sources cs ON cs.id = p.categorised_by_id
         WHERE a.type IN ('expense', 'income')",
    )
    .fetch_all(&db)
    .await
    .map_err(|e| e.to_string())?;
    let mut by_transaction: HashMap<i64, Vec<&CategoryRow>> = HashMap::new();
    for row in &rows {
        by_transaction.entry(row.0).or_default().push(row);
    }
    let single: Vec<_> = by_transaction
        .values()
        .filter(|v| v.len() == 1)
        .map(|v| v[0])
        .collect();
    println!("Transactions with one category: {}", single.len());
    let kept: Vec<Example> = single
        .into_iter()
        .filter(|r| {
            r.3 != UNCATEGORISED
                && (trust.contains(&r.4) || (trust.contains("confirmed") && r.5 == "confirmed"))
        })
        .map(|r| (r.1.clone(), r.2.clone(), r.3.clone()))
        .collect();
    let mut trusting: Vec<&String> = trust.iter().collect();
    trusting.sort();
    let trusting: Vec<&str> = trusting.iter().map(|s| s.as_str()).collect();
    println!("Using {} (trusting: {})", kept.len(), trusting.join(", "));
    Ok(kept)
}

fn load_csv(path: &str) -> Result<Vec<Example>, String> {
    let mut reader =
        csv::Reader::from_path(path).map_err(|e| format!("Couldn't read {path}: {e}"))?;
    let headers = reader.headers().map_err(|e| e.to_string())?.clone();
    let column = |name: &str| headers.iter().position(|h| h == name);
    let (Some(description), Some(category)) = (column("description"), column("category")) else {
        return Err("The CSV needs description and category columns.".into());
    };
    let date = column("date");
    let mut rows = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|e| e.to_string())?;
        let get = |i: Option<usize>| i.and_then(|i| record.get(i)).unwrap_or_default().to_owned();
        let (d, c) = (get(Some(description)), get(Some(category)));
        if !d.is_empty() && !c.is_empty() {
            rows.push((get(date), d, c));
        }
    }
    println!("Using {} rows from {path}", rows.len());
    Ok(rows)
}

/// The first two words naming the merchant, to tell merchants seen in training from new ones.
fn merchant_key(text: &str) -> String {
    clean(text)
        .split_whitespace()
        .filter(|w| !PREFIXES.contains(w) && w.len() > 1)
        .take(2)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Trains on `train` (category labels numbered) and predicts each of `test`:
/// (predicted category, confidence), or None if it can't train.
fn predict(train: &[&Example], test: &[&Example]) -> Option<Vec<(String, f64)>> {
    let mut labels: Vec<&str> = train.iter().map(|e| e.2.as_str()).collect();
    labels.sort_unstable();
    labels.dedup();
    let number: HashMap<&str, i64> = labels
        .iter()
        .enumerate()
        .map(|(i, l)| (*l, i as i64))
        .collect();
    let examples: Vec<(String, i64)> = train
        .iter()
        .map(|e| (e.1.clone(), number[e.2.as_str()]))
        .collect();
    // The app needs 30 examples; the evaluation trains on whatever it's given.
    let model = Model::train_any(&examples)?;
    Some(
        test.iter()
            .map(|e| {
                let (label, p) = model.predict(&e.1);
                (labels[label as usize].to_owned(), p)
            })
            .collect(),
    )
}

fn percent(numerator: usize, denominator: usize) -> String {
    if denominator == 0 {
        "   -  ".into()
    } else {
        format!("{:5.1}%", 100.0 * numerator as f64 / denominator as f64)
    }
}

/// Overall accuracy, split by merchants seen in training or new.
fn summary(correct: &[bool], new: &[bool]) -> String {
    let count = |pick: &dyn Fn(usize) -> bool| {
        let chosen: Vec<usize> = (0..correct.len()).filter(|i| pick(*i)).collect();
        (chosen.iter().filter(|i| correct[**i]).count(), chosen.len())
    };
    let (all_right, all) = count(&|_| true);
    let (seen_right, seen) = count(&|i| !new[i]);
    let (new_right, fresh) = count(&|i| new[i]);
    format!(
        "all {}   seen merchants {}   new merchants {}",
        percent(all_right, all),
        percent(seen_right, seen),
        percent(new_right, fresh)
    )
}

fn time_split(data: &[Example], fraction: f64) {
    let mut sorted: Vec<&Example> = data.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let cut = (sorted.len() as f64 * fraction) as usize;
    let (train, test) = sorted.split_at(cut);
    let seen: HashSet<String> = train.iter().map(|e| merchant_key(&e.1)).collect();
    let new: Vec<bool> = test
        .iter()
        .map(|e| !seen.contains(&merchant_key(&e.1)))
        .collect();
    println!(
        "\nTime split: train {}, test {} ({} from merchants not seen in training)",
        train.len(),
        test.len(),
        new.iter().filter(|n| **n).count()
    );
    let Some(predictions) = predict(train, test) else {
        println!("  Too few categories in the training part.");
        return;
    };
    let correct: Vec<bool> = predictions
        .iter()
        .zip(test)
        .map(|((p, _), e)| *p == e.2)
        .collect();
    println!("  learned model              {}", summary(&correct, &new));
    let sure: Vec<usize> = (0..test.len())
        .filter(|i| predictions[*i].1 >= CONFIDENCE)
        .collect();
    let right = sure.iter().filter(|i| correct[**i]).count();
    println!(
        "                             conf>={CONFIDENCE}: {} right on {} (what the app categorises itself)",
        percent(right, sure.len()),
        percent(sure.len(), test.len())
    );
}

fn cross_check(data: &[Example], folds: usize) {
    let n = data.len();
    let keys: Vec<String> = data.iter().map(|e| merchant_key(&e.1)).collect();
    let groups: Vec<Vec<usize>> = if folds == 0 {
        (0..n).map(|i| vec![i]).collect()
    } else {
        let mut order: Vec<usize> = (0..n).collect();
        order.shuffle(&mut rand::rngs::StdRng::seed_from_u64(0));
        order
            .chunks(n.div_ceil(folds))
            .map(<[usize]>::to_vec)
            .collect()
    };
    let mut correct = vec![false; n];
    let mut new = vec![false; n];
    let (mut sure, mut sure_right) = (0, 0);
    for group in &groups {
        let held: HashSet<usize> = group.iter().copied().collect();
        let train: Vec<&Example> = (0..n)
            .filter(|i| !held.contains(i))
            .map(|i| &data[i])
            .collect();
        let seen: HashSet<&String> = (0..n)
            .filter(|i| !held.contains(i))
            .map(|i| &keys[i])
            .collect();
        let test: Vec<&Example> = group.iter().map(|i| &data[*i]).collect();
        for i in group {
            new[*i] = !seen.contains(&keys[*i]);
        }
        let Some(predictions) = predict(&train, &test) else {
            continue;
        };
        for (i, (predicted, p)) in group.iter().zip(predictions) {
            correct[*i] = predicted == data[*i].2;
            if p >= CONFIDENCE {
                sure += 1;
                sure_right += usize::from(correct[*i]);
            }
        }
    }
    let label = if folds == 0 {
        "Leave-one-out".to_owned()
    } else {
        format!("{folds}-fold cross-check")
    };
    println!(
        "\n{label} over {n} ({} from merchants missing from their training data)",
        new.iter().filter(|x| **x).count()
    );
    println!("  learned model              {}", summary(&correct, &new));
    println!(
        "                             conf>={CONFIDENCE}: {} right on {}",
        percent(sure_right, sure),
        percent(sure, n)
    );
}
