//! Serving the React app, which is built into `frontend/dist` and embedded in the binary.
//!
//! Files that exist are served as they are. Any other page address gets `index.html`, so the
//! app's own router can show it. Server routes (`/api`, `/mcp`, OAuth) never fall back to the
//! app: a missing one is a 404.

use std::borrow::Cow;

use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "../../frontend/dist"]
#[allow_missing = true]
struct Dist;

/// Addresses that browsers and connector directories look for by convention.
const ICON_ALIASES: &[(&str, &str)] = &[
    ("favicon.ico", "pwa-192x192.png"),
    ("apple-touch-icon.png", "pwa-192x192.png"),
];

/// Paths that belong to the server, never to the app.
const SERVER_PREFIXES: &[&str] = &["api/", "mcp", "oauth/", ".well-known/"];

/// The app's own pages among them: where connectors send the user to approve them.
const APP_PAGES: &[&str] = &["oauth/authorize"];

pub async fn serve(uri: Uri) -> Response {
    respond(uri.path(), |path| Dist::get(path).map(|file| file.data))
}

/// What to send for `path`, finding built files with `lookup`.
fn respond(path: &str, lookup: impl Fn(&str) -> Option<Cow<'static, [u8]>>) -> Response {
    let path = path.trim_start_matches('/');

    if SERVER_PREFIXES
        .iter()
        .any(|prefix| path.starts_with(prefix))
        && !APP_PAGES.contains(&path)
    {
        return not_found();
    }

    let file = ICON_ALIASES
        .iter()
        .find(|(alias, _)| *alias == path)
        .map_or(path, |(_, target)| target);
    if !file.is_empty() {
        if let Some(body) = lookup(file) {
            return asset(file, body);
        }
        // Built files under assets/ are only ever asked for by name; a missing one is a 404
        // rather than the app's page, which the browser would try to run as a script.
        if file.starts_with("assets/") {
            return not_found();
        }
    }

    match lookup("index.html") {
        Some(body) => page(body),
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            "PennyChest's frontend hasn't been built. Run `npm run build` in frontend/, then \
             rebuild the server.",
        )
            .into_response(),
    }
}

fn asset(path: &str, body: Cow<'static, [u8]>) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    // Vite puts a content hash in every filename under assets/, so they never change.
    let cache = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    (
        [
            (header::CONTENT_TYPE, mime.as_ref().to_owned()),
            (header::CACHE_CONTROL, cache.to_owned()),
        ],
        body,
    )
        .into_response()
}

fn page(body: Cow<'static, [u8]>) -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            // Always check for a new build, so an update reaches the browser straight away.
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body,
    )
        .into_response()
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        axum::Json(serde_json::json!({ "detail": "Not Found" })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    const INDEX: &[u8] = b"<html>app</html>";

    fn built(path: &str) -> Option<Cow<'static, [u8]>> {
        match path {
            "index.html" => Some(Cow::Borrowed(INDEX)),
            "pwa-192x192.png" => Some(Cow::Borrowed(b"png")),
            "assets/index-abc.js" => Some(Cow::Borrowed(b"js")),
            _ => None,
        }
    }

    fn unbuilt(_: &str) -> Option<Cow<'static, [u8]>> {
        None
    }

    fn header(response: &Response, name: header::HeaderName) -> &str {
        response.headers()[name].to_str().unwrap()
    }

    #[test]
    fn serves_the_app_for_the_root_and_its_pages() {
        for path in ["/", "/transactions", "/settings/ai", "/imports/3/review"] {
            let response = respond(path, built);
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            assert_eq!(
                header(&response, header::CONTENT_TYPE),
                "text/html; charset=utf-8"
            );
            assert_eq!(header(&response, header::CACHE_CONTROL), "no-cache");
        }
    }

    #[test]
    fn serves_built_files_with_their_type() {
        let response = respond("/assets/index-abc.js", built);
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(header(&response, header::CONTENT_TYPE), "text/javascript");
        assert!(header(&response, header::CACHE_CONTROL).contains("immutable"));
    }

    #[test]
    fn serves_icon_aliases() {
        let response = respond("/favicon.ico", built);
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(header(&response, header::CONTENT_TYPE), "image/png");
    }

    #[test]
    fn missing_built_files_are_not_found() {
        assert_eq!(
            respond("/assets/gone.js", built).status(),
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn server_routes_never_fall_back_to_the_app() {
        for path in [
            "/api/nothing",
            "/mcp",
            "/oauth/token",
            "/.well-known/oauth-authorization-server",
        ] {
            assert_eq!(
                respond(path, built).status(),
                StatusCode::NOT_FOUND,
                "{path}"
            );
        }
    }

    #[test]
    fn the_consent_page_is_the_app() {
        let response = respond("/oauth/authorize", built);
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn says_when_the_frontend_is_not_built() {
        assert_eq!(
            respond("/", unbuilt).status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
}
