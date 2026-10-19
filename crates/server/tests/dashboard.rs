//! The dashboard layout. The cash flow and net worth figures are tested with the reports.

mod common;

use axum::http::StatusCode;
use common::TestApp;
use serde_json::{Value, json};

#[tokio::test]
async fn layout_defaults_saves_and_resets() {
    let mut app = TestApp::signed_in().await;
    let default = app.get("/api/dashboard/layout").await.body["widgets"].clone();
    let types: Vec<&str> = default
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["type"].as_str().unwrap())
        .collect();
    assert_eq!(types[..4], ["stat"; 4]);

    let layout = json!({
        "widgets": [
            {"id": "a", "type": "net_worth", "settings": {"months": 24}},
            {
                "id": "b",
                "type": "spending_trend",
                "settings": {"months": 12, "chart": "line", "categories": ["Expenses:Food"]}
            },
            {"id": "c", "type": "unmatched_taps"}
        ]
    });
    app.put("/api/dashboard/layout", layout)
        .await
        .assert_status(StatusCode::OK);
    let saved = app.get("/api/dashboard/layout").await.body;
    let ids: Vec<&str> = saved["widgets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["a", "b", "c"]);
    assert_eq!(
        saved["widgets"][1]["settings"]["categories"],
        json!(["Expenses:Food"])
    );

    let reset = app.delete("/api/dashboard/layout").await;
    assert_eq!(reset.body, json!({ "widgets": default }));
    let after = app.get("/api/dashboard/layout").await;
    assert_eq!(after.body, json!({ "widgets": default }));
}

#[tokio::test]
async fn layouts_are_checked() {
    let mut app = TestApp::signed_in().await;
    for bad in [
        json!({"widgets": [{"id": "a", "type": "pie_of_everything"}]}),
        json!({"widgets": [{"id": "a", "type": "stat", "settings": {"stat": "mood"}}]}),
        json!({"widgets": [{"id": "a", "type": "net_worth", "settings": {"colour": "red"}}]}),
        json!({"widgets": [{"id": "a", "type": "net_worth", "settings": {"months": 1000}}]}),
    ] {
        let response = app.put("/api/dashboard/layout", bad.clone()).await;
        response.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
        // A sentence the frontend can show, not FastAPI's list of objects.
        assert!(
            response.body["detail"].is_string(),
            "{bad}: {}",
            response.body
        );
    }
}

#[tokio::test]
async fn unset_settings_come_back_as_null() {
    let mut app = TestApp::signed_in().await;
    let saved = app
        .put(
            "/api/dashboard/layout",
            json!({"widgets": [{"id": "a", "type": "pending"}]}),
        )
        .await;
    saved.assert_status(StatusCode::OK);
    let null = Value::Null;
    let settings = &saved.body["widgets"][0]["settings"];
    for key in ["stat", "period", "months", "chart", "categories"] {
        assert_eq!(settings.get(key), Some(&null), "{key}");
    }
}

#[tokio::test]
async fn bodies_that_arent_json_are_refused_with_a_sentence() {
    let mut app = TestApp::signed_in().await;
    let response = app
        .send(axum::http::Method::PUT, "/api/dashboard/layout", None)
        .await;
    response.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    assert!(response.body["detail"].as_str().unwrap().contains("JSON"));
}

#[tokio::test]
async fn layout_needs_a_session() {
    let mut app = TestApp::new().await;
    app.get("/api/dashboard/layout")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}
