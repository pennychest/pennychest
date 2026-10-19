//! The insight actions, with unusual charges and price rises read from what the insights model
//! stores.

mod common;
mod fixtures;

use axum::http::StatusCode;
use chrono::NaiveDate;
use common::TestApp;
use fixtures::{Ids, act, ledger, ok, spend, spending};
use pennychest_core::insights::{self, Detail};
use serde_json::{Value, json};

const OTHER: &str = "Expenses:Other";
const CURRENT: &str = "Assets:Bank:Current";

/// Regular charges on top of the spending fixture.
async fn subscriptions(app: &mut TestApp, ids: &Ids) {
    for day in [
        "2026-05-15",
        "2026-06-15",
        "2026-07-15",
        "2026-08-15",
        "2026-09-15",
    ] {
        let description = format!("NETFLIX.COM {}88", &day[5..7]);
        spend(app, ids, day, &description, OTHER, "10.99", CURRENT, true).await;
    }
    // Cancelled
    for day in ["2026-03-01", "2026-04-01", "2026-05-01"] {
        spend(app, ids, day, "PUREGYM", OTHER, "25.00", CURRENT, true).await;
    }
    for day in ["2025-10-01", "2026-09-25"] {
        spend(
            app,
            ids,
            day,
            "ACME INSURANCE",
            OTHER,
            "240.00",
            CURRENT,
            true,
        )
        .await;
    }
}

async fn with_subscriptions() -> TestApp {
    let mut app = TestApp::signed_in().await;
    let ids = ledger(&mut app).await;
    spending(&mut app, &ids).await;
    subscriptions(&mut app, &ids).await;
    app
}

fn rows<'a>(list: &'a Value, fields: &[&str]) -> Vec<Vec<&'a Value>> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|item| fields.iter().map(|f| &item[*f]).collect())
        .collect()
}

#[tokio::test]
async fn recurring_payments() {
    let mut app = with_subscriptions().await;
    let result = ok(&mut app, "recurring_payments", json!({"on": "2026-09-30"})).await;
    assert_eq!(
        rows(
            &result["payments"],
            &["merchant", "cadence", "yearly_cost", "kind"]
        ),
        [
            [
                &json!("ACME INSURANCE"),
                &json!("yearly"),
                &json!("240.00"),
                &json!("unlabelled")
            ],
            [
                &json!("NETFLIX.COM 0988"),
                &json!("monthly"),
                &json!("131.88"),
                &json!("unlabelled")
            ],
        ]
    );
    let netflix = &result["payments"][1];
    assert_eq!(netflix["charges"], 5);
    assert_eq!(netflix["category"], OTHER);
    assert_eq!(netflix["amount"], "10.99");
    assert_eq!(netflix["amount_varies"], false);
    assert_eq!(result["yearly_total"], "371.88");
    // Tesco only appears twice, and the gym stopped in May.

    sqlx::query(
        "INSERT INTO merchant_labels (merchant_key, kind) VALUES ('netflix com', 'subscription')",
    )
    .execute(&app.db)
    .await
    .unwrap();
    let subscriptions = ok(
        &mut app,
        "recurring_payments",
        json!({"on": "2026-09-30", "kind": "subscription"}),
    )
    .await;
    assert_eq!(
        rows(&subscriptions["payments"], &["merchant", "kind"]),
        [[&json!("NETFLIX.COM 0988"), &json!("subscription")]]
    );
    assert_eq!(subscriptions["yearly_total"], "131.88");
    let limited = ok(
        &mut app,
        "recurring_payments",
        json!({"on": "2026-09-30", "limit": 1}),
    )
    .await;
    assert_eq!(limited["payments_not_shown"], 1);
    assert_eq!(limited["yearly_total"], "371.88");
}

#[tokio::test]
async fn compare_periods() {
    let mut app = with_subscriptions().await;
    let result = ok(
        &mut app,
        "compare_periods",
        json!({"start_date": "2026-08-01", "end_date": "2026-08-31", "category_depth": 2}),
    )
    .await;
    assert_eq!(result["compared_with"]["start_date"], "2026-07-01");
    assert_eq!(result["compared_with"]["end_date"], "2026-07-31");
    assert_eq!(result["change"], "95.00"); // 145.99 - 50.99
    assert_eq!(
        rows(
            &result["groups"],
            &["key", "total", "compared_total", "change"]
        ),
        [
            [
                &json!("Expenses:Food"),
                &json!("102.00"),
                &json!("40.00"),
                &json!("62.00")
            ],
            [
                &json!("Expenses:Transport"),
                &json!("33.00"),
                &json!("0.00"),
                &json!("33.00")
            ],
            [
                &json!("Expenses:Other"),
                &json!("10.99"),
                &json!("10.99"),
                &json!("0.00")
            ],
        ]
    );
    let explicit = ok(
        &mut app,
        "compare_periods",
        json!({
            "start_date": "2026-08-01",
            "end_date": "2026-08-31",
            "compare_start_date": "2026-05-01",
            "compare_end_date": "2026-05-31",
            "group_by": "merchant",
            "limit": 1,
        }),
    )
    .await;
    // Netflix and the gym
    assert_eq!(explicit["compared_with"]["total"], "35.99");
    assert_eq!(explicit["groups"][0]["key"], "TESCO STORES 1");
    for bad in [
        json!({"start_date": "2026-08-01", "end_date": "2026-08-31", "compare_start_date": "2026-07-01"}),
        json!({"start_date": "2026-08-31", "end_date": "2026-08-01"}),
        json!({"start_date": "2026-08-01", "end_date": "2026-08-31", "compare_start_date": "2026-07-31", "compare_end_date": "2026-07-01"}),
        json!({"start_date": "2026-08-01", "end_date": "2026-08-31", "group_by": "month"}),
    ] {
        act(&mut app, "compare_periods", bad)
            .await
            .assert_status(StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn savings_opportunities() {
    let mut app = with_subscriptions().await;
    ok(
        &mut app,
        "set_budget",
        json!({"category": OTHER, "amount": "5"}),
    )
    .await;
    let result = ok(
        &mut app,
        "savings_opportunities",
        json!({"months": 2, "on": "2026-09-28"}),
    )
    .await;
    assert_eq!(
        result["averaged_over"],
        json!({"start_date": "2026-07-01", "end_date": "2026-08-31", "months": 2})
    );
    // (50.99 + 145.99) / 2
    assert_eq!(result["monthly_average_spending"], "98.49");
    assert_eq!(result["unreviewed"], "10.00");
    assert_eq!(
        result["top_categories"].as_array().unwrap()[..2],
        [
            json!({"category": "Expenses:Food", "monthly_average": "71.00"}),
            json!({"category": "Expenses:Transport", "monthly_average": "16.50"}),
        ]
    );
    assert_eq!(
        result["top_merchants"][0],
        json!({"merchant": "TESCO STORES 1", "monthly_average": "47.75", "transactions": 2})
    );
    // August against the June-July average; Other didn't change so isn't listed.
    assert_eq!(
        result["rising_last_month"],
        json!({
            "month": "2026-08",
            "categories": [
                {"category": "Expenses:Food", "last_month": "102.00", "usual_month": "20.00", "increase": "82.00"},
                {"category": "Expenses:Transport", "last_month": "33.00", "usual_month": "0.00", "increase": "33.00"},
            ],
        })
    );
    let merchants: Vec<&Value> = result["recurring_payments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| &p["merchant"])
        .collect();
    assert_eq!(
        merchants,
        [&json!("ACME INSURANCE"), &json!("NETFLIX.COM 0988")]
    );
    assert_eq!(result["recurring_yearly_total"], "371.88");
    assert_eq!(
        result["overspent_budgets"],
        json!([{
            "category": OTHER,
            "period": "monthly",
            "limit": "5.00",
            "spent": "250.99",
            "over_by": "245.99",
        }])
    );
}

#[tokio::test]
async fn the_digest_lists_every_transaction() {
    let mut app = TestApp::signed_in().await;
    let ids = ledger(&mut app).await;
    spending(&mut app, &ids).await;
    let result = ok(
        &mut app,
        "transactions_digest",
        json!({"start_date": "2026-08-01", "end_date": "2026-08-31"}),
    )
    .await;
    assert_eq!(result["detail"], "transactions");
    assert_eq!(result["transactions"], 7); // the transfer to savings is left out
    assert_eq!(result["total_spending"], "135.00");
    assert_eq!(result["total_income"], "2000.00");
    assert_eq!(result["unreviewed"], "10.00");
    let csv = result["csv"].as_str().unwrap();
    let lines: Vec<&str> = csv.lines().collect();
    assert_eq!(lines[0], "date,description,amount,category,account,status");
    assert_eq!(
        lines[1],
        "2026-08-03,TESCO STORES 1,55.50,Expenses:Food:Groceries,Assets:Bank:Current,confirmed"
    );
    assert!(!csv.contains("\"PIZZA 100% REAL\"")); // only quoted when needed
    assert!(lines.contains(
        &"2026-08-30,SUPERMARKET + TRAIN,20.00,Expenses:Food:Groceries;Expenses:Transport,\
          Assets:Bank:Current,confirmed"
    ));
    assert!(csv.contains("2026-08-28,ACME PAYROLL,-2000.00,Income:Salary"));
    assert!(!csv.contains("TO SAVINGS"));
}

#[tokio::test]
async fn the_digest_rolls_up_by_merchant() {
    let app = with_subscriptions().await;
    let day = |s: &str| s.parse::<NaiveDate>().unwrap();
    // Five at most, so the 15 transactions are rolled up.
    let result = insights::transactions_digest(
        &app.db,
        day("2026-05-01"),
        day("2026-09-30"),
        Detail::Auto,
        5,
    )
    .await
    .unwrap();
    assert_eq!(result["detail"], "merchants");
    let csv = result["csv"].as_str().unwrap();
    let lines: Vec<&str> = csv.lines().collect();
    assert_eq!(
        lines[0],
        "merchant,category,transactions,total,typical_amount,first_date,last_date"
    );
    assert_eq!(
        lines[1],
        "ACME INSURANCE,Expenses:Other,1,240.00,240.00,2026-09-25,2026-09-25"
    );
    assert!(lines.contains(&"NETFLIX.COM 0988,Expenses:Other,5,54.95,10.99,2026-05-15,2026-09-15"));
    assert!(
        lines
            .last()
            .unwrap()
            .starts_with("ACME PAYROLL,Income:Salary,1,-2000.00")
    );

    let mut app = app;
    let forced = ok(
        &mut app,
        "transactions_digest",
        json!({"start_date": "2026-05-01", "end_date": "2026-09-30", "detail": "transactions"}),
    )
    .await;
    assert_eq!(forced["detail"], "transactions");
}

#[tokio::test]
async fn unusual_charges_and_price_rises() {
    let mut app = TestApp::signed_in().await;
    let ids = ledger(&mut app).await;
    for (day, amount) in [
        ("2026-06-01", "30.00"),
        ("2026-07-01", "30.00"),
        ("2026-08-01", "30.00"),
        ("2026-09-01", "34.50"),
    ] {
        spend(
            &mut app,
            &ids,
            day,
            "OCTOPUS ENERGY",
            "Expenses:Other",
            amount,
            CURRENT,
            true,
        )
        .await;
    }
    let big = spend(
        &mut app,
        &ids,
        "2026-09-10",
        "ELECTRONICS WORLD",
        "Expenses:Other",
        "899.00",
        CURRENT,
        true,
    )
    .await;
    let odd = spend(
        &mut app,
        &ids,
        "2026-09-12",
        "CORNER SHOP",
        "Expenses:Food:Groceries",
        "60.00",
        CURRENT,
        true,
    )
    .await;
    spend(
        &mut app,
        &ids,
        "2026-09-14",
        "REFUND ACME",
        "Expenses:Other",
        "-20.00",
        CURRENT,
        true,
    )
    .await;
    for (id, score) in [(big, 1.0), (odd, 0.5)] {
        sqlx::query("UPDATE transactions SET unusual_score = ? WHERE id = ?")
            .bind(score)
            .bind(id)
            .execute(&app.db)
            .await
            .unwrap();
    }

    let result = ok(
        &mut app,
        "unusual_charges",
        json!({"start_date": "2026-09-01", "end_date": "2026-09-30"}),
    )
    .await;
    assert_eq!(
        rows(
            &result["unusual_charges"],
            &["description", "amount", "unusual_score", "category"]
        ),
        [
            [
                &json!("ELECTRONICS WORLD"),
                &json!("899.00"),
                &json!(1.0),
                &json!("Expenses:Other")
            ],
            [
                &json!("CORNER SHOP"),
                &json!("60.00"),
                &json!(0.5),
                &json!("Expenses:Food:Groceries")
            ],
        ]
    );
    // The energy bill isn't scored; the refund isn't a charge.
    assert_eq!(result["unscored_charges"], 1);
    assert_eq!(
        result["price_rises"],
        json!([{
            "merchant": "OCTOPUS ENERGY",
            "kind": "unlabelled",
            "cadence": "monthly",
            "usual_amount": "30.00",
            "latest_amount": "34.50",
            "latest_date": "2026-09-01",
            "increase": "4.50",
        }])
    );

    // Ordinary spending that happens to repeat isn't a price rise.
    sqlx::query(
        "INSERT INTO merchant_labels (merchant_key, kind) VALUES ('octopus energy', 'other')",
    )
    .execute(&app.db)
    .await
    .unwrap();
    let result = ok(
        &mut app,
        "unusual_charges",
        json!({"start_date": "2026-09-01", "end_date": "2026-09-30", "limit": 1}),
    )
    .await;
    assert_eq!(result["price_rises"], json!([]));
    assert_eq!(result["unusual_not_shown"], 1);
    act(
        &mut app,
        "unusual_charges",
        json!({"start_date": "2026-09-30", "end_date": "2026-09-01"}),
    )
    .await
    .assert_status(StatusCode::BAD_REQUEST);
}
