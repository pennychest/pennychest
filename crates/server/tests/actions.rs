//! The actions endpoints. The insight actions' tests are in `insights.rs`.

mod common;
mod fixtures;

use std::collections::{HashMap, HashSet};

use axum::http::StatusCode;
use chrono::Local;
use common::TestApp;
use fixtures::{act, imported, ledger, ok, postings, spending, statement_row};
use pennychest_core::actions::{self, ActionError, Caller, Scope, Source};
use serde_json::{Value, json};

const READ: [&str; 14] = [
    "list_categories",
    "list_accounts",
    "spending_summary",
    "list_transactions",
    "list_rules",
    "get_budgets",
    "list_card_taps",
    "cash_flow",
    "net_worth_history",
    "recurring_payments",
    "unusual_charges",
    "compare_periods",
    "savings_opportunities",
    "transactions_digest",
];
const ORGANISE: [&str; 5] = [
    "create_category",
    "rename_category",
    "create_rule",
    "set_budget",
    "delete_budget",
];
const TRANSACTIONS: [&str; 6] = [
    "recategorise_transactions",
    "add_transaction",
    "add_transfer",
    "edit_transaction",
    "delete_transactions",
    "mark_reviewed",
];

fn set(names: &[&[&str]]) -> HashSet<String> {
    names
        .iter()
        .flat_map(|n| n.iter())
        .map(|n| (*n).to_owned())
        .collect()
}

fn id_list(list: &Value) -> Vec<i64> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_i64().unwrap())
        .collect()
}

fn paths(categories: &Value) -> Vec<String> {
    categories["categories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["path"].as_str().unwrap().to_owned())
        .collect()
}

fn caller(scopes: &[Scope]) -> Caller<'_> {
    Caller {
        scopes,
        source: Source::Api,
        conversation_id: None,
    }
}

// Catalogue

#[tokio::test]
async fn the_catalogue_lists_every_action_with_strict_schemas() {
    let mut app = TestApp::signed_in().await;
    let listed = app.get("/api/actions").await.body;
    let actions: HashMap<String, Value> = listed["actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| (a["name"].as_str().unwrap().to_owned(), a.clone()))
        .collect();
    let names: HashSet<String> = actions.keys().cloned().collect();
    assert_eq!(names, set(&[&READ, &ORGANISE, &TRANSACTIONS]));
    for (name, action) in &actions {
        let expected = if ORGANISE.contains(&name.as_str()) {
            "organise"
        } else if TRANSACTIONS.contains(&name.as_str()) {
            "transactions"
        } else {
            "read"
        };
        assert_eq!(action["scope"], expected, "{name}");
        assert!(!action["description"].as_str().unwrap().trim().is_empty());
        let schema = &action["input_schema"];
        assert_eq!(schema["type"], "object", "{name}");
        assert_eq!(schema["additionalProperties"], false, "{name}");
    }
}

#[tokio::test]
async fn actions_require_a_session() {
    let mut app = TestApp::new().await;
    app.get("/api/actions")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    act(&mut app, "list_categories", json!({}))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn bad_calls_are_explained() {
    let mut app = TestApp::signed_in().await;
    ledger(&mut app).await;
    act(&mut app, "nope", json!({}))
        .await
        .assert_status(StatusCode::NOT_FOUND);
    let response = act(&mut app, "list_categories", json!({"colour": "blue"})).await;
    response.assert_status(StatusCode::BAD_REQUEST);
    assert!(response.body["detail"].as_str().unwrap().contains("colour"));
    let response = act(
        &mut app,
        "spending_summary",
        json!({"start_date": "yesterday", "end_date": "2026-01-01"}),
    )
    .await;
    response.assert_status(StatusCode::BAD_REQUEST);
    assert!(
        response.body["detail"]
            .as_str()
            .unwrap()
            .contains("start_date")
    );
    let response = act(
        &mut app,
        "spending_summary",
        json!({"start_date": "2026-02-01", "end_date": "2026-01-01"}),
    )
    .await;
    response.assert_status(StatusCode::BAD_REQUEST);
    assert!(
        response.body["detail"]
            .as_str()
            .unwrap()
            .contains("end_date")
    );
    let response = act(&mut app, "list_categories", json!([1, 2])).await;
    response.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
}

// Reading

#[tokio::test]
async fn list_categories_and_accounts() {
    let mut app = TestApp::signed_in().await;
    ledger(&mut app).await;
    let categories = paths(&ok(&mut app, "list_categories", json!({})).await);
    assert!(categories.contains(&"Expenses:Food:Groceries".into()));
    assert!(categories.contains(&"Income:Salary".into()));
    assert!(!categories.contains(&"Assets:Bank:Current".into()));
    let income = ok(&mut app, "list_categories", json!({"kind": "income"})).await;
    assert!(
        income["categories"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["type"] == "income")
    );
    let accounts = ok(&mut app, "list_accounts", json!({})).await;
    let accounts: Vec<&str> = accounts["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["path"].as_str().unwrap())
        .collect();
    assert_eq!(
        accounts,
        [
            "Assets:Bank:Current",
            "Assets:Bank:Savings",
            "Liabilities:Visa"
        ]
    );
}

#[tokio::test]
async fn spending_summaries() {
    let mut app = TestApp::signed_in().await;
    let ids = ledger(&mut app).await;
    spending(&mut app, &ids).await;
    let august = json!({"start_date": "2026-08-01", "end_date": "2026-08-31"});
    let summary = ok(&mut app, "spending_summary", august.clone()).await;
    assert_eq!(summary["total"], "135.00");
    assert_eq!(summary["unreviewed"], "10.00");
    assert_eq!(
        summary["groups"][0],
        json!({"key": "Expenses:Food:Groceries", "total": "72.00", "transactions": 4})
    );

    let rolled = ok(
        &mut app,
        "spending_summary",
        json!({"start_date": "2026-08-01", "end_date": "2026-08-31", "category_depth": 2}),
    )
    .await;
    assert_eq!(
        rolled["groups"],
        json!([
            {"key": "Expenses:Food", "total": "102.00", "transactions": 5},
            {"key": "Expenses:Transport", "total": "33.00", "transactions": 2},
        ])
    );

    let income = ok(
        &mut app,
        "spending_summary",
        json!({"start_date": "2026-08-01", "end_date": "2026-08-31", "kind": "income"}),
    )
    .await;
    assert_eq!(income["total"], "2000.00");
    assert_eq!(
        income["groups"],
        json!([{"key": "Income:Salary", "total": "2000.00", "transactions": 1}])
    );
    act(
        &mut app,
        "spending_summary",
        json!({"start_date": "2026-08-01", "end_date": "2026-08-31", "limit": 0}),
    )
    .await
    .assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn cash_flow_and_net_worth_for_the_dashboard() {
    let mut app = TestApp::signed_in().await;
    let ids = ledger(&mut app).await;
    spending(&mut app, &ids).await;
    let flow = ok(
        &mut app,
        "cash_flow",
        json!({"months": 3, "end": "2026-09-15"}),
    )
    .await;
    assert_eq!(
        flow["months"],
        json!([
            {"month": "2026-07", "income": "0.00", "expenses": "40.00", "net": "-40.00"},
            {"month": "2026-08", "income": "2000.00", "expenses": "135.00", "net": "1865.00"},
            {"month": "2026-09", "income": "0.00", "expenses": "0.00", "net": "0.00"},
        ])
    );
    assert_eq!(flow["expenses_total"], "175.00");
    let worth = ok(
        &mut app,
        "net_worth_history",
        json!({"months": 4, "end": "2026-09-15"}),
    )
    .await;
    assert_eq!(
        worth["currencies"]["GBP"],
        json!([
            {"date": "2026-06-30", "net_worth": "0.00"},
            {"date": "2026-07-31", "net_worth": "-40.00"},
            {"date": "2026-08-31", "net_worth": "1825.00"},
            {"date": "2026-09-15", "net_worth": "1825.00"},
        ])
    );
    act(&mut app, "cash_flow", json!({"months": 61}))
        .await
        .assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn list_transactions_filters() {
    let mut app = TestApp::signed_in().await;
    let ids = ledger(&mut app).await;
    let s = spending(&mut app, &ids).await;
    let everything = ok(&mut app, "list_transactions", json!({})).await;
    assert_eq!(everything["total"], 9);
    // Newest first
    assert_eq!(
        everything["transactions"][0]["description"],
        "SUPERMARKET + TRAIN"
    );

    let pizza = ok(&mut app, "list_transactions", json!({"search": "100%"})).await;
    assert_eq!(
        pizza["transactions"],
        json!([{
            "id": s["pizza"],
            "date": "2026-08-10",
            "description": "PIZZA 100% REAL",
            "category": "Expenses:Food:Restaurants",
            "amount": "30.00",
            "account": "Liabilities:Visa",
            "status": "confirmed",
            "imported": false,
        }])
    );
    let tesco = ok(&mut app, "list_transactions", json!({"search": "tesco"})).await;
    assert_eq!(tesco["total"], 3);

    let food = ok(
        &mut app,
        "list_transactions",
        json!({"category": "Expenses:Food"}),
    )
    .await;
    assert_eq!(food["total"], 6);
    let exact = ok(
        &mut app,
        "list_transactions",
        json!({"category": "Expenses:Food", "exact_category": true}),
    )
    .await;
    assert_eq!(exact["total"], 0);
    let visa = ok(&mut app, "list_transactions", json!({"account": "Visa"})).await;
    assert_eq!(ids_of(&visa), [s["pizza"]]);
    let pending = ok(&mut app, "list_transactions", json!({"status": "pending"})).await;
    assert_eq!(ids_of(&pending), [s["pending"]]);
    let big = ok(
        &mut app,
        "list_transactions",
        json!({"min_amount": "30", "max_amount": "60"}),
    )
    .await;
    let big: HashSet<i64> = ids_of(&big).into_iter().collect();
    assert_eq!(
        big,
        HashSet::from([s["tesco_jul"], s["tesco_aug"], s["pizza"]])
    );
    let august = ok(
        &mut app,
        "list_transactions",
        json!({"start_date": "2026-08-01", "end_date": "2026-08-15", "limit": 2, "offset": 1}),
    )
    .await;
    assert_eq!(august["total"], 3);
    assert_eq!(august["transactions"].as_array().unwrap().len(), 2);

    let transfer = everything["transactions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == s["transfer"])
        .unwrap();
    assert_eq!(transfer["category"], Value::Null);
    assert_eq!(transfer["amount"], "0.00");
}

fn ids_of(result: &Value) -> Vec<i64> {
    id_list(&result["transactions"])
}

#[tokio::test]
async fn category_names_resolve_or_explain() {
    let mut app = TestApp::signed_in().await;
    let ids = ledger(&mut app).await;
    spending(&mut app, &ids).await;
    let groceries = ok(
        &mut app,
        "list_transactions",
        json!({"category": "groceries"}),
    )
    .await;
    assert_eq!(groceries["total"], 5);
    let response = act(&mut app, "list_transactions", json!({"category": "Other"})).await;
    response.assert_status(StatusCode::BAD_REQUEST);
    let detail = response.body["detail"].as_str().unwrap();
    assert!(detail.contains("Expenses:Other") && detail.contains("Income:Other"));
    let response = act(&mut app, "list_transactions", json!({"category": "Bank"})).await;
    response.assert_status(StatusCode::BAD_REQUEST);
    assert!(
        response.body["detail"]
            .as_str()
            .unwrap()
            .contains("list_categories")
    );
}

#[tokio::test]
async fn card_taps() {
    let mut app = TestApp::signed_in().await;
    ledger(&mut app).await;
    sqlx::query(
        "INSERT INTO card_taps (tapped_on, merchant, amount, dismissed) VALUES
         ('2026-08-01', 'Pret', 42000, 0), ('2026-08-02', 'Boots', 99900, 1)",
    )
    .execute(&app.db)
    .await
    .unwrap();
    let unmatched = ok(&mut app, "list_card_taps", json!({})).await;
    let unmatched: Vec<(&str, &str)> = unmatched["taps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            (
                t["merchant"].as_str().unwrap(),
                t["amount"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(unmatched, [("Pret", "4.20")]);
    let all = ok(&mut app, "list_card_taps", json!({"status": "all"})).await;
    assert_eq!(all["taps"].as_array().unwrap().len(), 2);
    let dismissed = ok(&mut app, "list_card_taps", json!({"status": "dismissed"})).await;
    assert_eq!(dismissed["taps"][0]["merchant"], "Boots");
}

// Changing data

#[tokio::test]
async fn create_category() {
    let mut app = TestApp::signed_in().await;
    ledger(&mut app).await;
    let created = ok(
        &mut app,
        "create_category",
        json!({"name": "Amazon Prime", "parent": "Food"}),
    )
    .await;
    assert_eq!(
        created["created"],
        json!({"path": "Expenses:Food:AmazonPrime", "name": "Amazon Prime", "type": "expense"})
    );
    let all = paths(&ok(&mut app, "list_categories", json!({})).await);
    assert!(all.contains(&"Expenses:Food:AmazonPrime".into()));

    for bad in [
        json!({"name": "Amazon  Prime", "parent": "Food"}),
        json!({"name": "!!!", "parent": "Food"}),
        json!({"name": "Pocket", "parent": "Assets:Bank"}),
        json!({"name": "", "parent": "Food"}),
    ] {
        act(&mut app, "create_category", bad)
            .await
            .assert_status(StatusCode::BAD_REQUEST);
    }
    let income = ok(
        &mut app,
        "create_category",
        json!({"name": "Dividends", "parent": "Income"}),
    )
    .await;
    assert_eq!(income["created"]["type"], "income");
}

#[tokio::test]
async fn renaming_a_category_moves_its_subcategories() {
    let mut app = TestApp::signed_in().await;
    let ids = ledger(&mut app).await;
    spending(&mut app, &ids).await;
    ok(
        &mut app,
        "set_budget",
        json!({"category": "Food", "amount": "300"}),
    )
    .await;
    let renamed = ok(
        &mut app,
        "rename_category",
        json!({"category": "Expenses:Food", "new_name": "Food & Drink"}),
    )
    .await;
    assert_eq!(
        renamed["renamed"],
        json!({"from": "Expenses:Food", "to": "Expenses:FoodDrink", "subcategories_moved": 2})
    );
    let expenses = paths(&ok(&mut app, "list_categories", json!({"kind": "expense"})).await);
    assert!(expenses.contains(&"Expenses:FoodDrink:Groceries".into()));
    assert!(!expenses.contains(&"Expenses:Food:Groceries".into()));
    let food = ok(
        &mut app,
        "list_transactions",
        json!({"category": "Expenses:FoodDrink"}),
    )
    .await;
    assert_eq!(food["total"], 6);
    let budgets = ok(&mut app, "get_budgets", json!({"on": "2026-08-15"})).await;
    assert_eq!(budgets["budgets"][0]["category"], "Expenses:FoodDrink");

    for bad in [
        json!({"category": "Uncategorised", "new_name": "Misc"}),
        json!({"category": "Expenses", "new_name": "Costs"}),
        json!({"category": "Transport", "new_name": "FoodDrink"}),
    ] {
        act(&mut app, "rename_category", bad)
            .await
            .assert_status(StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn create_rule() {
    let mut app = TestApp::signed_in().await;
    ledger(&mut app).await;
    let created = ok(
        &mut app,
        "create_rule",
        json!({"pattern": "DELIVEROO", "category": "Restaurants", "description": "Food delivery"}),
    )
    .await["created"]
        .clone();
    assert_eq!(created["category"], "Expenses:Food:Restaurants");
    assert_eq!(created["priority"], 10);
    let rules = ok(&mut app, "list_rules", json!({})).await;
    assert_eq!(
        rules["rules"],
        json!([{
            "id": created["id"],
            "pattern": "DELIVEROO",
            "match_type": "substring",
            "category": "Expenses:Food:Restaurants",
            "priority": 10,
            "description": "Food delivery",
        }])
    );
    assert_eq!(app.get("/api/rules").await.body[0]["source"], "ai");

    act(
        &mut app,
        "create_rule",
        json!({"pattern": "DELIVEROO", "category": "Restaurants"}),
    )
    .await
    .assert_status(StatusCode::BAD_REQUEST);
    let response = act(
        &mut app,
        "create_rule",
        json!({"pattern": "(", "category": "Restaurants", "match_type": "regex"}),
    )
    .await;
    response.assert_status(StatusCode::BAD_REQUEST);
    assert!(
        response.body["detail"]
            .as_str()
            .unwrap()
            .contains("regular expression")
    );
    act(
        &mut app,
        "create_rule",
        json!({"pattern": "X", "category": "Restaurants", "priority": 0}),
    )
    .await
    .assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn recategorise_transactions() {
    let mut app = TestApp::signed_in().await;
    let ids = ledger(&mut app).await;
    let s = spending(&mut app, &ids).await;
    let result = ok(
        &mut app,
        "recategorise_transactions",
        json!({
            "category": "Transport",
            "transaction_ids": [s["tesco_aug"], s["transfer"], s["split"], 999999, s["tesco_aug"]],
        }),
    )
    .await;
    assert_eq!(result["updated"], json!([s["tesco_aug"]]));
    assert_eq!(
        result["skipped"],
        json!([
            {"id": s["transfer"], "reason": "has no category (e.g. a transfer)"},
            {"id": s["split"], "reason": "split across several categories"},
            {"id": 999999, "reason": "not found"},
        ])
    );
    let moved = postings(&mut app, s["tesco_aug"]).await;
    assert!(moved.contains_key("Expenses:Transport"));
    act(
        &mut app,
        "recategorise_transactions",
        json!({"category": "Transport", "transaction_ids": []}),
    )
    .await
    .assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn set_and_delete_budgets() {
    let mut app = TestApp::signed_in().await;
    let ids = ledger(&mut app).await;
    spending(&mut app, &ids).await;
    let first = ok(
        &mut app,
        "set_budget",
        json!({"category": "Food", "amount": "250"}),
    )
    .await;
    assert_eq!(
        first["budget"],
        json!({"category": "Expenses:Food", "period": "monthly", "limit": "250.00"})
    );
    assert_eq!(first["previous_limit"], Value::Null);
    let second = ok(
        &mut app,
        "set_budget",
        json!({"category": "Food", "amount": "300"}),
    )
    .await;
    assert_eq!(second["previous_limit"], "250.00");
    ok(
        &mut app,
        "set_budget",
        json!({"category": "Transport", "amount": "1000", "period": "annual"}),
    )
    .await;

    let budgets = ok(&mut app, "get_budgets", json!({"on": "2026-08-15"})).await;
    let by_category: HashMap<&str, &Value> = budgets["budgets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| (b["category"].as_str().unwrap(), b))
        .collect();
    let food = by_category["Expenses:Food"];
    assert_eq!(
        (&food["spent"], &food["remaining"], &food["unreviewed"]),
        (&json!("102.00"), &json!("198.00"), &json!("10.00"))
    );
    assert_eq!(
        by_category["Expenses:Transport"]["period_start"],
        "2026-01-01"
    );
    assert_eq!(
        app.get("/api/budgets").await.body.as_array().unwrap().len(),
        2
    );

    act(
        &mut app,
        "set_budget",
        json!({"category": "Salary", "amount": "5"}),
    )
    .await
    .assert_status(StatusCode::BAD_REQUEST);
    act(
        &mut app,
        "set_budget",
        json!({"category": "Food", "amount": "0"}),
    )
    .await
    .assert_status(StatusCode::BAD_REQUEST);
    let deleted = ok(&mut app, "delete_budget", json!({"category": "Food"})).await;
    assert_eq!(deleted["deleted"]["limit"], "300.00");
    act(&mut app, "delete_budget", json!({"category": "Food"}))
        .await
        .assert_status(StatusCode::BAD_REQUEST);
}

// Scopes

#[tokio::test]
async fn callers_are_read_only_by_default() {
    let mut app = TestApp::signed_in().await;
    ledger(&mut app).await;
    let refused = actions::run(
        &app.db,
        "create_category",
        json!({"name": "Gym", "parent": "Expenses"}),
        caller(&[]),
    )
    .await
    .unwrap_err();
    assert!(matches!(refused, ActionError::NotAllowed(_)));
    assert!(refused.to_string().contains("'organise' permission"));
    let categories = actions::run(&app.db, "list_categories", json!({}), caller(&[]))
        .await
        .unwrap();
    assert!(!paths(&Value::Object(categories)).contains(&"Expenses:Gym".into()));
}

#[tokio::test]
async fn each_changing_action_needs_its_own_scope() {
    let app = TestApp::signed_in().await;
    // Refused before the input is even looked at.
    for (names, needed, others) in [
        (&ORGANISE[..], "'organise'", [Scope::Transactions]),
        (&TRANSACTIONS[..], "'transactions'", [Scope::Organise]),
    ] {
        for name in names {
            for scopes in [&[][..], &others[..]] {
                let refused = actions::run(&app.db, name, json!({}), caller(scopes))
                    .await
                    .unwrap_err();
                assert!(refused.to_string().contains(needed), "{name}");
            }
        }
    }
}

#[tokio::test]
async fn the_organise_and_transactions_scopes_are_separate() {
    let mut app = TestApp::signed_in().await;
    let ids = ledger(&mut app).await;
    let s = spending(&mut app, &ids).await;
    let organiser = caller(&[Scope::Organise]);
    let created = actions::run(
        &app.db,
        "create_category",
        json!({"name": "Gym", "parent": "Expenses"}),
        organiser,
    )
    .await
    .unwrap();
    assert_eq!(created["created"]["path"], "Expenses:Gym");
    assert!(matches!(
        actions::run(
            &app.db,
            "recategorise_transactions",
            json!({"transaction_ids": [s["train"]], "category": "Gym"}),
            organiser,
        )
        .await,
        Err(ActionError::NotAllowed(_))
    ));

    let bookkeeper = caller(&[Scope::Transactions]);
    let moved = actions::run(
        &app.db,
        "recategorise_transactions",
        json!({"transaction_ids": [s["train"]], "category": "Groceries"}),
        bookkeeper,
    )
    .await
    .unwrap();
    assert_eq!(moved["updated"], json!([s["train"]]));
    assert!(matches!(
        actions::run(
            &app.db,
            "set_budget",
            json!({"category": "Food", "amount": "10"}),
            bookkeeper,
        )
        .await,
        Err(ActionError::NotAllowed(_))
    ));
}

#[tokio::test]
async fn the_catalogue_is_filtered_by_scope() {
    let names = |scopes: &[Scope]| -> HashSet<String> {
        actions::describe(scopes)
            .iter()
            .map(|a| a["name"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(names(&[]), set(&[&READ]));
    assert_eq!(names(&[Scope::Organise]), set(&[&READ, &ORGANISE]));
    assert_eq!(names(&[Scope::Transactions]), set(&[&READ, &TRANSACTIONS]));
    assert_eq!(names(&Scope::ALL), set(&[&READ, &ORGANISE, &TRANSACTIONS]));
    assert!("admin".parse::<Scope>().is_err());
}

#[tokio::test]
async fn a_signed_in_session_has_every_scope() {
    let mut app = TestApp::signed_in().await;
    let ids = ledger(&mut app).await;
    let s = spending(&mut app, &ids).await;
    ok(
        &mut app,
        "create_category",
        json!({"name": "Gym", "parent": "Expenses"}),
    )
    .await;
    ok(
        &mut app,
        "recategorise_transactions",
        json!({"transaction_ids": [s["train"]], "category": "Gym"}),
    )
    .await;
}

// Transactions

fn as_numbers(postings: HashMap<String, String>) -> HashMap<String, f64> {
    postings
        .into_iter()
        .map(|(path, amount)| (path, amount.parse().unwrap()))
        .collect()
}

fn numbers(pairs: &[(&str, f64)]) -> HashMap<String, f64> {
    pairs.iter().map(|(p, a)| ((*p).to_owned(), *a)).collect()
}

#[tokio::test]
async fn add_spending_from_a_card() {
    let mut app = TestApp::signed_in().await;
    ledger(&mut app).await;
    let added = ok(
        &mut app,
        "add_transaction",
        json!({
            "description": "Cash coffee",
            "amount": "4.50",
            "category": "Restaurants",
            "account": "Visa",
            "date": "2026-08-20",
        }),
    )
    .await["added"]
        .clone();
    assert_eq!(
        added,
        json!({
            "id": added["id"],
            "date": "2026-08-20",
            "description": "Cash coffee",
            "category": "Expenses:Food:Restaurants",
            "amount": "4.50",
            "account": "Liabilities:Visa",
            "status": "confirmed",
            "imported": false,
        })
    );
    let id = added["id"].as_i64().unwrap();
    assert_eq!(
        as_numbers(postings(&mut app, id).await),
        numbers(&[
            ("Expenses:Food:Restaurants", 4.5),
            ("Liabilities:Visa", -4.5)
        ])
    );
    let summary = ok(
        &mut app,
        "spending_summary",
        json!({"start_date": "2026-08-20", "end_date": "2026-08-20"}),
    )
    .await;
    assert_eq!(
        (&summary["total"], &summary["unreviewed"]),
        (&json!("4.50"), &json!("0.00"))
    );
}

#[tokio::test]
async fn add_income_a_refund_and_the_default_date() {
    let mut app = TestApp::signed_in().await;
    ledger(&mut app).await;
    let income = ok(
        &mut app,
        "add_transaction",
        json!({"description": "Birthday money", "amount": "50", "category": "Income:Other", "account": "Current"}),
    )
    .await["added"]
        .clone();
    assert_eq!(income["amount"], "-50.00");
    assert_eq!(income["date"], Local::now().date_naive().to_string());
    let id = income["id"].as_i64().unwrap();
    assert_eq!(
        as_numbers(postings(&mut app, id).await)["Assets:Bank:Current"],
        50.0
    );

    let ambiguous = act(
        &mut app,
        "add_transaction",
        json!({"description": "Returned shoes", "amount": "-30", "category": "Other", "account": "Current"}),
    )
    .await;
    ambiguous.assert_status(StatusCode::BAD_REQUEST);
    assert!(
        ambiguous.body["detail"]
            .as_str()
            .unwrap()
            .contains("Expenses:Other")
    );
    let found = ok(
        &mut app,
        "list_transactions",
        json!({"search": "Returned shoes"}),
    )
    .await;
    assert_eq!(found["total"], 0);
}

#[tokio::test]
async fn add_transaction_validation() {
    let mut app = TestApp::signed_in().await;
    ledger(&mut app).await;
    let base =
        json!({"description": "x", "amount": "5", "category": "Groceries", "account": "Current"});
    let with = |key: &str, value: &str| {
        let mut call = base.clone();
        call[key] = json!(value);
        call
    };
    for bad in [
        with("amount", "0"),
        with("category", "Current"),
        with("account", "Groceries"),
        with("amount", "1.23456"),
    ] {
        act(&mut app, "add_transaction", bad)
            .await
            .assert_status(StatusCode::BAD_REQUEST);
    }
    let refund = ok(&mut app, "add_transaction", with("amount", "-5")).await["added"].clone();
    assert_eq!(refund["amount"], "-5.00");
    let id = refund["id"].as_i64().unwrap();
    assert_eq!(
        as_numbers(postings(&mut app, id).await)["Assets:Bank:Current"],
        5.0
    );
}

#[tokio::test]
async fn add_a_transfer() {
    let mut app = TestApp::signed_in().await;
    ledger(&mut app).await;
    let added = ok(
        &mut app,
        "add_transfer",
        json!({"amount": "100", "from_account": "Current", "to_account": "Savings", "date": "2026-08-31"}),
    )
    .await["added"]
        .clone();
    assert_eq!(added["category"], Value::Null);
    assert_eq!(added["description"], "Transfer");
    let id = added["id"].as_i64().unwrap();
    assert_eq!(
        as_numbers(postings(&mut app, id).await),
        numbers(&[
            ("Assets:Bank:Current", -100.0),
            ("Assets:Bank:Savings", 100.0)
        ])
    );
    for bad in [
        json!({"amount": "5", "from_account": "Current", "to_account": "Current"}),
        json!({"amount": "-5", "from_account": "Current", "to_account": "Savings"}),
    ] {
        act(&mut app, "add_transfer", bad)
            .await
            .assert_status(StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn edit_a_transaction_the_user_added() {
    let mut app = TestApp::signed_in().await;
    ledger(&mut app).await;
    let txn = ok(
        &mut app,
        "add_transaction",
        json!({"description": "Coffee", "amount": "3", "category": "Restaurants", "account": "Current", "date": "2026-08-01"}),
    )
    .await["added"]
        .clone();
    let id = txn["id"].as_i64().unwrap();
    let edited = ok(
        &mut app,
        "edit_transaction",
        json!({"transaction_id": id, "description": "Lunch", "date": "2026-08-02", "amount": "12.40", "account": "Visa"}),
    )
    .await["edited"]
        .clone();
    assert_eq!(
        (
            &edited["description"],
            &edited["date"],
            &edited["amount"],
            &edited["account"]
        ),
        (
            &json!("Lunch"),
            &json!("2026-08-02"),
            &json!("12.40"),
            &json!("Liabilities:Visa")
        )
    );
    assert_eq!(
        as_numbers(postings(&mut app, id).await),
        numbers(&[
            ("Expenses:Food:Restaurants", 12.4),
            ("Liabilities:Visa", -12.4)
        ])
    );

    // Moving from spending to income keeps the amount's meaning and flips the entry.
    let moved = ok(
        &mut app,
        "edit_transaction",
        json!({"transaction_id": id, "category": "Salary"}),
    )
    .await["edited"]
        .clone();
    assert_eq!(moved["category"], "Income:Salary");
    assert_eq!(moved["amount"], "-12.40");
    assert_eq!(
        as_numbers(postings(&mut app, id).await)["Liabilities:Visa"],
        12.4
    );
}

#[tokio::test]
async fn edit_transfers_and_splits() {
    let mut app = TestApp::signed_in().await;
    let ids = ledger(&mut app).await;
    let s = spending(&mut app, &ids).await;
    ok(
        &mut app,
        "edit_transaction",
        json!({"transaction_id": s["transfer"], "amount": "75"}),
    )
    .await;
    assert_eq!(
        as_numbers(postings(&mut app, s["transfer"]).await),
        numbers(&[
            ("Assets:Bank:Current", -75.0),
            ("Assets:Bank:Savings", 75.0)
        ])
    );
    for bad in [
        json!({"transaction_id": s["transfer"], "category": "Groceries"}),
        json!({"transaction_id": s["transfer"], "amount": "-5"}),
        json!({"transaction_id": s["split"], "amount": "30"}),
        json!({"transaction_id": 999999, "amount": "1"}),
    ] {
        act(&mut app, "edit_transaction", bad)
            .await
            .assert_status(StatusCode::BAD_REQUEST);
    }
    // A refused edit changes nothing.
    assert_eq!(
        as_numbers(postings(&mut app, s["transfer"]).await)["Assets:Bank:Savings"],
        75.0
    );
    let renamed = ok(
        &mut app,
        "edit_transaction",
        json!({"transaction_id": s["split"], "description": "Weekly shop"}),
    )
    .await;
    assert_eq!(renamed["edited"]["description"], "Weekly shop");
}

#[tokio::test]
async fn imported_transactions_can_be_edited() {
    let mut app = TestApp::signed_in().await;
    let ids = ledger(&mut app).await;
    let id = imported(&mut app, &ids).await;
    let edited = ok(
        &mut app,
        "edit_transaction",
        json!({
            "transaction_id": id,
            "amount": "25",
            "date": "2026-08-06",
            "description": "Tesco groceries",
            "account": "Visa",
            "category": "Transport",
        }),
    )
    .await;
    assert_eq!(
        edited["edited"],
        json!({
            "id": id,
            "date": "2026-08-06",
            "description": "Tesco groceries",
            "category": "Expenses:Transport",
            "amount": "25.00",
            "account": "Liabilities:Visa",
            "status": "pending",
            "imported": true,
        })
    );
    // What the bank reported is still stored with the import.
    assert_eq!(statement_row(&app).await.0, Some(id));
}

#[tokio::test]
async fn descriptions_steer_models_away_from_bank_details() {
    let edit = actions::find("edit_transaction").unwrap().description;
    assert!(
        edit.contains("only change their amount, date, account or description when the user asks")
    );
    let delete = actions::find("delete_transactions").unwrap().description;
    assert!(delete.contains("only delete one when the user asks for it"));
}

#[tokio::test]
async fn delete_transactions() {
    let mut app = TestApp::signed_in().await;
    let ids = ledger(&mut app).await;
    let s = spending(&mut app, &ids).await;
    let statement = imported(&mut app, &ids).await;
    let manual = ok(
        &mut app,
        "add_transaction",
        json!({"description": "Oops", "amount": "1", "category": "Groceries", "account": "Current"}),
    )
    .await["added"]["id"]
        .as_i64()
        .unwrap();
    let result = ok(
        &mut app,
        "delete_transactions",
        json!({"transaction_ids": [manual, s["transfer"], statement, 999999]}),
    )
    .await;
    assert_eq!(
        id_list(&result["deleted"]),
        [manual, s["transfer"], statement]
    );
    assert_eq!(
        result["skipped"],
        json!([{"id": 999999, "reason": "not found"}])
    );
    for id in [manual, s["transfer"], statement] {
        app.get(&format!("/api/transactions/{id}"))
            .await
            .assert_status(StatusCode::NOT_FOUND);
    }
    // The statement row stays with its import.
    assert_eq!(statement_row(&app).await, (None, Some(1)));
}

#[tokio::test]
async fn the_app_can_delete_imported_transactions() {
    let mut app = TestApp::signed_in().await;
    let ids = ledger(&mut app).await;
    let id = imported(&mut app, &ids).await;
    app.delete(&format!("/api/transactions/{id}"))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    assert_eq!(statement_row(&app).await.0, None);
}

#[tokio::test]
async fn mark_reviewed() {
    let mut app = TestApp::signed_in().await;
    let ids = ledger(&mut app).await;
    let s = spending(&mut app, &ids).await;
    let statement = imported(&mut app, &ids).await;
    let result = ok(
        &mut app,
        "mark_reviewed",
        json!({"transaction_ids": [statement, s["pending"], s["pizza"]]}),
    )
    .await;
    assert_eq!(result["reviewed"], json!([statement, s["pending"]]));
    assert_eq!(
        result["skipped"],
        json!([{"id": s["pizza"], "reason": "already reviewed"}])
    );
    let pending = ok(&mut app, "list_transactions", json!({"status": "pending"})).await;
    assert_eq!(pending["total"], 0);
}
