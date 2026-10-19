//! Errors as the API reports them: a status and `{"detail": "..."}`, which the frontend shows
//! to the user. `detail` is always a sentence, even for a request the server can't accept.

use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequest, Request};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde::de::DeserializeOwned;

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub detail: String,
}

impl ApiError {
    pub fn new(status: StatusCode, detail: impl Into<String>) -> Self {
        Self {
            status,
            detail: detail.into(),
        }
    }

    pub fn not_found(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, detail)
    }

    pub fn bad_request(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, detail)
    }

    /// The request was understood but its contents aren't acceptable.
    pub fn invalid(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, detail)
    }

    /// Something went wrong on the server. The cause is logged, not shown.
    pub fn internal(cause: impl std::fmt::Display) -> Self {
        tracing::error!("{cause}");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Something went wrong on the server. The details are in its log.",
        )
    }
}

/// How every error is answered.
#[derive(Serialize, utoipa::ToSchema)]
pub struct ErrorBody {
    /// What went wrong, as a sentence to show the user.
    pub detail: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            axum::Json(ErrorBody {
                detail: self.detail,
            }),
        )
            .into_response()
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(err: sqlx::Error) -> Self {
        Self::internal(err)
    }
}

impl From<pennychest_core::settings::SettingError> for ApiError {
    fn from(err: pennychest_core::settings::SettingError) -> Self {
        Self::internal(err)
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

/// A JSON request body. Like axum's `Json`, but a body that's missing, malformed or the wrong
/// shape is a 422 with a readable `detail`, as FastAPI answered.
pub struct JsonBody<T>(pub T);

impl<S, T> FromRequest<S> for JsonBody<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, ApiError> {
        match axum::Json::<T>::from_request(req, state).await {
            Ok(axum::Json(value)) => Ok(Self(value)),
            Err(rejection) => Err(ApiError::invalid(explain(&rejection))),
        }
    }
}

fn explain(rejection: &JsonRejection) -> String {
    match rejection {
        JsonRejection::JsonDataError(err) => {
            // e.g. "widgets[0].type: unknown variant `pie`, expected one of ..."
            let detail =
                std::error::Error::source(err).map_or_else(|| err.body_text(), ToString::to_string);
            format!("The request isn't valid: {detail}")
        }
        JsonRejection::JsonSyntaxError(_) => "The request isn't valid JSON.".into(),
        JsonRejection::MissingJsonContentType(_) => {
            "The request must be JSON, sent as application/json.".into()
        }
        other => other.body_text(),
    }
}

/// A value from the path, like an id. One that can't be read is a 422 with a readable
/// `detail`, as FastAPI answered, rather than axum's plain-text 400.
pub struct PathParam<T>(pub T);

impl<S, T> axum::extract::FromRequestParts<S> for PathParam<T>
where
    T: DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &S,
    ) -> Result<Self, ApiError> {
        match axum::extract::Path::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Path(value)) => Ok(Self(value)),
            Err(rejection) => Err(ApiError::invalid(format!(
                "The address isn't valid: {}",
                rejection.body_text()
            ))),
        }
    }
}

/// Query string parameters. Ones that can't be read are a 422 with a readable `detail`, as
/// FastAPI answered, rather than axum's plain-text 400.
pub struct QueryParams<T>(pub T);

impl<S, T> axum::extract::FromRequestParts<S> for QueryParams<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &S,
    ) -> Result<Self, ApiError> {
        match axum::extract::Query::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Query(value)) => Ok(Self(value)),
            Err(rejection) => Err(ApiError::invalid(format!(
                "The query isn't valid: {}",
                rejection.body_text()
            ))),
        }
    }
}

/// A multipart form: its file (if any) and its text fields, as FastAPI's `File` and `Form`.
pub struct Form {
    pub file: Option<UploadedFile>,
    pub fields: std::collections::HashMap<String, String>,
}

pub struct UploadedFile {
    pub filename: String,
    pub content: Vec<u8>,
}

impl<S: Send + Sync> FromRequest<S> for Form {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, ApiError> {
        let mut multipart = axum::extract::Multipart::from_request(req, state)
            .await
            .map_err(|e| {
                ApiError::invalid(format!("The request must be a form: {}", e.body_text()))
            })?;
        let mut form = Self {
            file: None,
            fields: std::collections::HashMap::new(),
        };
        while let Some(field) = multipart.next_field().await.map_err(|e| {
            ApiError::invalid(format!("The form couldn't be read: {}", e.body_text()))
        })? {
            let name = field.name().unwrap_or_default().to_owned();
            if name == "file" {
                let filename = field.file_name().unwrap_or_default().to_owned();
                let content = field.bytes().await.map_err(|e| {
                    ApiError::invalid(format!("The file couldn't be read: {}", e.body_text()))
                })?;
                form.file = Some(UploadedFile {
                    filename,
                    content: content.to_vec(),
                });
            } else {
                let value = field.text().await.map_err(|e| {
                    ApiError::invalid(format!("The form couldn't be read: {}", e.body_text()))
                })?;
                form.fields.insert(name, value);
            }
        }
        Ok(form)
    }
}

impl Form {
    /// The uploaded file, which the request must have.
    pub fn require_file(&mut self) -> Result<UploadedFile, ApiError> {
        self.file
            .take()
            .ok_or_else(|| ApiError::invalid("The request must include a file."))
    }

    /// A true/false field, read as FastAPI read them; missing is false.
    pub fn flag(&self, name: &str) -> Result<bool, ApiError> {
        match self
            .fields
            .get(name)
            .map(|v| v.trim().to_ascii_lowercase())
            .as_deref()
        {
            None | Some("" | "false" | "0" | "no" | "off" | "f" | "n") => Ok(false),
            Some("true" | "1" | "yes" | "on" | "t" | "y") => Ok(true),
            Some(other) => Err(ApiError::invalid(format!(
                "{name} must be true or false, not {other:?}"
            ))),
        }
    }
}
