//! The API's OpenAPI description, served at `/api/openapi.json` and committed as
//! `openapi.json`, from which the frontend's types are generated
//! (`frontend/src/api/schema.ts`).
//!
//! Routes join it as they're described: add `#[utoipa::path]` to the handler, list it in
//! `paths` below, and run `UPDATE_OPENAPI=1 cargo test -p pennychest-server --test openapi`
//! and then `npm run api:types` in `frontend`.

use axum::http::header;
use axum::response::IntoResponse;
use utoipa::OpenApi;

use crate::error::ErrorBody;
use crate::routes::auth;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "PennyChest",
        description = "The JSON API behind PennyChest's web app. Everything but health and signing in needs \
                       the session cookie that signing in sets."
    ),
    paths(
        crate::health,
        auth::status,
        auth::setup,
        auth::login,
        auth::logout,
        auth::change_password,
    ),
    components(schemas(ErrorBody)),
    tags(
        (name = "health", description = "Whether the server is up"),
        (name = "auth", description = "The password, and signing in and out"),
    )
)]
struct ApiDoc;

/// The description as JSON, formatted as it's committed.
pub fn json() -> String {
    let mut json = ApiDoc::openapi()
        .to_pretty_json()
        .expect("the OpenAPI description serialises");
    json.push('\n');
    json
}

pub(crate) async fn serve() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "application/json")], json())
}
