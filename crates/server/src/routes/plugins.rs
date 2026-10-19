//! `/api/plugins`: Settings > Plugins. The plugins each repository offers, and installing
//! and removing them.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::future::join_all;
use pennychest_core::plugins::index::{self, Index, OFFICIAL, REPOSITORIES};
use pennychest_core::plugins::install::InstallError;
use pennychest_core::settings::Json as Stored;
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::error::{ApiError, ApiResult, JsonBody, QueryParams};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list))
        .route(
            "/repositories",
            post(add_repository).delete(remove_repository),
        )
        .route("/install", post(install))
        .route("/uninstall", post(uninstall))
}

/// Every repository plugins can be installed from, the official one first.
async fn repositories(state: &AppState) -> ApiResult<Vec<String>> {
    let mut urls = vec![OFFICIAL.to_owned()];
    if state.config.allow_plugin_repositories {
        urls.extend(added(state).await?);
    }
    Ok(urls)
}

async fn added(state: &AppState) -> ApiResult<Vec<String>> {
    Ok(REPOSITORIES
        .get(&state.db)
        .await?
        .map(|s| s.0)
        .unwrap_or_default())
}

async fn fetch_index(state: &AppState, url: &str) -> Option<Index> {
    match index::fetch_index(state.http.as_ref(), url).await {
        Ok(index) => Some(index),
        Err(problem) => {
            tracing::warn!("Couldn't read the plugin list at {url}: {problem}");
            None
        }
    }
}

#[derive(Serialize)]
struct RepositoryPlugin {
    package: String,
    name: String,
    description: String,
    version: String,
    installed_version: Option<String>,
    /// Never, now: nothing is built in but the CSV importer. Kept for the page.
    built_in: bool,
    /// Whether there's a download for this machine.
    available: bool,
}

#[derive(Serialize)]
struct Repository {
    url: String,
    name: String,
    official: bool,
    error: Option<String>,
    plugins: Vec<RepositoryPlugin>,
}

#[derive(Serialize)]
struct Installed {
    package: String,
    version: String,
}

#[derive(Serialize)]
struct Broken {
    package: String,
    problem: String,
}

#[derive(Serialize)]
struct PluginList {
    repositories: Vec<Repository>,
    /// Installed from a repository that's since been removed, or that couldn't be read.
    other_installed: Vec<Installed>,
    /// Installed, but they couldn't be started.
    broken: Vec<Broken>,
    can_add_repositories: bool,
}

async fn list(State(state): State<AppState>) -> ApiResult<Json<PluginList>> {
    let urls = repositories(&state).await?;
    let indexes = join_all(urls.iter().map(|url| fetch_index(&state, url))).await;
    let installed = state.plugins.installed();
    let version_of = |package: &str| {
        installed
            .iter()
            .find(|p| p.name == package)
            .map(|p| p.version.clone())
    };
    let mut listed = Vec::new();
    let repositories = urls
        .into_iter()
        .zip(indexes)
        .map(|(url, index)| {
            let plugins = index
                .as_ref()
                .map(|index| {
                    index
                        .plugins
                        .iter()
                        .map(|plugin| {
                            listed.push(plugin.package.clone());
                            RepositoryPlugin {
                                package: plugin.package.clone(),
                                name: plugin.name.clone(),
                                description: plugin.description.clone(),
                                version: plugin.version.clone(),
                                installed_version: version_of(&plugin.package),
                                built_in: false,
                                available: plugin.download().is_some(),
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            Repository {
                official: url == OFFICIAL,
                name: index
                    .as_ref()
                    .map_or_else(|| url.clone(), |i| i.name.clone()),
                error: index
                    .is_none()
                    .then(|| format!("Couldn't read this repository's {}.", index::INDEX_FILE)),
                url,
                plugins,
            }
        })
        .collect();
    Ok(Json(PluginList {
        repositories,
        other_installed: installed
            .iter()
            .filter(|p| !listed.contains(&p.name))
            .map(|p| Installed {
                package: p.name.clone(),
                version: p.version.clone(),
            })
            .collect(),
        broken: state
            .plugins
            .broken()
            .into_iter()
            .map(|b| Broken {
                package: b.name,
                problem: b.problem,
            })
            .collect(),
        can_add_repositories: state.config.allow_plugin_repositories,
    }))
}

#[derive(Deserialize)]
struct RepositoryIn {
    url: String,
}

async fn add_repository(
    State(state): State<AppState>,
    JsonBody(body): JsonBody<RepositoryIn>,
) -> ApiResult<StatusCode> {
    if !state.config.allow_plugin_repositories {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "Adding plugin repositories is turned off.",
        ));
    }
    let url = index::normalise(&body.url).map_err(ApiError::bad_request)?;
    if repositories(&state).await?.contains(&url) {
        return Err(ApiError::bad_request("That repository is already added."));
    }
    if fetch_index(&state, &url).await.is_none() {
        return Err(ApiError::bad_request(format!(
            "Couldn't find a valid {} in that repository.",
            index::INDEX_FILE
        )));
    }
    let mut urls = added(&state).await?;
    urls.push(url);
    REPOSITORIES.set(&state.db, &Stored(urls)).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct RepositoryParam {
    url: String,
}

/// Plugins installed from it stay installed.
async fn remove_repository(
    State(state): State<AppState>,
    QueryParams(param): QueryParams<RepositoryParam>,
) -> ApiResult<StatusCode> {
    let urls: Vec<String> = added(&state)
        .await?
        .into_iter()
        .filter(|u| *u != param.url)
        .collect();
    REPOSITORIES.set(&state.db, &Stored(urls)).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct InstallIn {
    repository: String,
    package: String,
}

/// Installs or updates a plugin, from the download its repository lists for it now.
async fn install(
    State(state): State<AppState>,
    JsonBody(body): JsonBody<InstallIn>,
) -> ApiResult<StatusCode> {
    if !repositories(&state).await?.contains(&body.repository) {
        return Err(ApiError::bad_request("That repository isn't added."));
    }
    let index = fetch_index(&state, &body.repository).await.ok_or_else(|| {
        ApiError::new(
            StatusCode::BAD_GATEWAY,
            "Couldn't read the repository's plugin list.",
        )
    })?;
    let plugin = index
        .get(&body.package)
        .ok_or_else(|| ApiError::not_found("The repository doesn't list that plugin."))?;
    let name = plugin.name.clone();
    let download = plugin.download().ok_or_else(|| {
        ApiError::bad_request(format!(
            "{name} isn't available for this server ({}).",
            index::platform()
        ))
    })?;
    let archive = index::fetch(state.http.as_ref(), &download.url, 300)
        .await
        .map_err(|e| {
            ApiError::new(
                StatusCode::BAD_GATEWAY,
                format!("Couldn't download {name}: {e}."),
            )
        })?;
    let plugins = state.plugins.clone();
    let package = body.package.clone();
    tokio::task::spawn_blocking(move || plugins.install(&package, &archive, &download.sha256))
        .await
        .map_err(ApiError::internal)?
        .map_err(|e| ApiError::bad_request(format!("Couldn't install {name}: {e}.")))?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct UninstallIn {
    package: String,
}

async fn uninstall(
    State(state): State<AppState>,
    JsonBody(body): JsonBody<UninstallIn>,
) -> ApiResult<StatusCode> {
    let plugins = state.plugins.clone();
    let package = body.package;
    tokio::task::spawn_blocking(move || plugins.uninstall(&package))
        .await
        .map_err(ApiError::internal)?
        .map_err(|e| match e {
            InstallError::NotInstalled => {
                ApiError::not_found("That plugin isn't installed from Settings.")
            }
            other => ApiError::bad_request(format!("Couldn't remove it: {other}.")),
        })?;
    Ok(StatusCode::NO_CONTENT)
}
