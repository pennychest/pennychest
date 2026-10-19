//! The rules endpoints.

mod common;

use axum::http::StatusCode;
use common::TestApp;
use serde_json::{Value, json};

async fn seeded() -> TestApp {
    let mut app = TestApp::signed_in().await;
    app.post("/api/setup/seed", json!({})).await;
    app
}

async fn id_of(app: &mut TestApp, path: &str) -> i64 {
    let accounts = app.get("/api/accounts").await.body;
    accounts
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["full_path"] == path)
        .unwrap()["id"]
        .as_i64()
        .unwrap()
}

#[tokio::test]
async fn lists_rules_in_the_order_they_are_tried() {
    let mut app = seeded().await;
    let rules = app.get("/api/rules").await.body;
    let rules = rules.as_array().unwrap();
    assert_eq!(rules.len(), 20);
    let priorities: Vec<i64> = rules
        .iter()
        .map(|r| r["priority"].as_i64().unwrap())
        .collect();
    let mut sorted = priorities.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(priorities, sorted);
    assert_eq!(
        rules[0],
        json!({
            "id": 1, "pattern": "TESCO", "match_type_id": 1, "match_type_name": "substring",
            "target_account_id": rules[0]["target_account_id"],
            "target_account_path": "Expenses:Food:Groceries", "priority": 10,
            "source": "manual", "description": null
        })
    );
}

#[tokio::test]
async fn creates_reads_updates_and_deletes() {
    let mut app = seeded().await;
    let fuel = id_of(&mut app, "Expenses:Transport:Fuel").await;
    let created = app
        .post(
            "/api/rules",
            json!({"pattern": "SHELL", "match_type_id": 2, "target_account_id": fuel,
                   "priority": 5, "description": "Petrol"}),
        )
        .await;
    created.assert_status(StatusCode::CREATED);
    let id = created.body["id"].as_i64().unwrap();
    assert_eq!(created.body["match_type_name"], "prefix");
    assert_eq!(created.body["source"], "manual");
    assert_eq!(
        app.get(&format!("/api/rules/{id}")).await.body,
        created.body
    );

    let updated = app
        .put(
            &format!("/api/rules/{id}"),
            json!({"priority": 50, "description": null}),
        )
        .await;
    assert_eq!(updated.body["priority"], 50);
    assert_eq!(updated.body["description"], Value::Null);
    assert_eq!(updated.body["pattern"], "SHELL");

    app.delete(&format!("/api/rules/{id}"))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    app.get(&format!("/api/rules/{id}"))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn refuses_bad_rules_rather_than_failing() {
    let mut app = seeded().await;
    let food = id_of(&mut app, "Expenses:Food").await;
    for (body, detail) in [
        (
            json!({"pattern": "X", "match_type_id": 9, "target_account_id": food}),
            "Match type 9 not found",
        ),
        (
            json!({"pattern": "X", "match_type_id": 1, "target_account_id": 999}),
            "Account 999 not found",
        ),
    ] {
        let response = app.post("/api/rules", body).await;
        response.assert_status(StatusCode::BAD_REQUEST);
        assert_eq!(response.body["detail"], detail);
    }
    let regex = app
        .post(
            "/api/rules",
            json!({"pattern": "(", "match_type_id": 3, "target_account_id": food}),
        )
        .await;
    regex.assert_status(StatusCode::BAD_REQUEST);
    assert!(
        regex.body["detail"]
            .as_str()
            .unwrap()
            .starts_with("That pattern isn't a regular expression")
    );
}

#[tokio::test]
async fn tests_and_previews_patterns() {
    let mut app = seeded().await;
    let tested = app
        .post(
            "/api/rules/test",
            json!({"pattern": "tesco", "match_type_id": 1,
                   "descriptions": ["TESCO STORES", "Asda", "tesco express"]}),
        )
        .await;
    assert_eq!(
        tested.body,
        json!({"matching": ["TESCO STORES", "tesco express"], "match_count": 2, "total": 3})
    );

    let current = id_of(&mut app, "Assets:Bank:Current").await;
    let groceries = id_of(&mut app, "Expenses:Food:Groceries").await;
    let txn = app
        .post(
            "/api/transactions",
            json!({"date": "2026-01-01", "description": "TESCO STORES", "postings": [
                {"account_id": groceries, "amount": "5", "categorised_by_id": 1},
                {"account_id": current, "amount": "-5", "categorised_by_id": 3}
            ]}),
        )
        .await
        .body;
    let preview = app
        .post(
            "/api/rules/preview",
            json!({"pattern": "TESCO", "match_type_id": 1}),
        )
        .await
        .body;
    assert_eq!(preview["match_count"], 1);
    assert_eq!(
        preview["matching_transactions"],
        json!([{"id": txn["id"], "description": "TESCO STORES"}])
    );
    assert_eq!(preview["conflicts"][0]["pattern"], "TESCO");
    let excluded = app
        .post(
            "/api/rules/preview",
            json!({"pattern": "TESCO", "match_type_id": 1, "exclude_rule_id": 1}),
        )
        .await
        .body;
    assert_eq!(excluded["conflicts"], json!([]));
}

#[tokio::test]
async fn apply_preview_looks_only_at_categories() {
    let mut app = seeded().await;
    let current = id_of(&mut app, "Assets:Bank:Current").await;
    let takeaway = id_of(&mut app, "Expenses:Food:Takeaway").await;
    let clothing = id_of(&mut app, "Expenses:Shopping:Clothing").await;
    // A Tesco shop filed under Takeaway by a rule, and one filed by hand under Clothing
    for (category, source) in [(takeaway, 1), (clothing, 2)] {
        app.post(
            "/api/transactions",
            json!({"date": "2026-01-01", "description": "TESCO STORES", "postings": [
                {"account_id": category, "amount": "5", "categorised_by_id": source},
                {"account_id": current, "amount": "-5", "categorised_by_id": 3}
            ]}),
        )
        .await;
    }
    let preview = app.post("/api/rules/apply-preview", json!({})).await.body;
    assert_eq!(preview["would_change_count"], 1);
    assert_eq!(preview["manual_conflict_count"], 1);
    let items = preview["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "the bank side is never listed: {preview}");
    assert!(
        items
            .iter()
            .all(|i| i["new_account_path"] == "Expenses:Food:Groceries")
    );
}

#[tokio::test]
async fn exports_and_imports() {
    let mut app = seeded().await;
    let exported = app.get("/api/rules/export").await.body;
    assert_eq!(
        exported[0],
        json!({"pattern": "TESCO", "match_type": "substring",
               "target_account": "Expenses:Food:Groceries", "priority": 10})
    );
    let again = app.post("/api/rules/import", exported.clone()).await;
    assert_eq!(again.body, json!({"imported": 0, "skipped": 20}));
    let new = app
        .post(
            "/api/rules/import",
            json!([{"pattern": "BOOTS", "target_account": "Expenses:PersonalCare"},
                   {"pattern": "X", "target_account": "Nowhere"}]),
        )
        .await;
    assert_eq!(new.body, json!({"imported": 1, "skipped": 1}));
}
