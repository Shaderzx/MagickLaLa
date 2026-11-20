use axum::Json;
use axum::extract::FromRequest;
use axum::extract::FromRequestParts;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use magicklala_graph::SpellError;
use magicklala_runtime::RuntimeError;
use magicklala_store::StoreError;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    error: &'a str,
    code: &'a str,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self { status, code, message: message.into() }
    }

    pub fn bad_request(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", message)
    }

    pub fn conflict(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, code, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(ErrorBody { error: &self.message, code: self.code })).into_response()
    }
}

impl From<StoreError> for ApiError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::NotFound(_) => Self::not_found(error.to_string()),
            StoreError::Conflict(_) => Self::conflict("conflict", error.to_string()),
            StoreError::Invalid(_) => Self::bad_request("invalid_input", error.to_string()),
            StoreError::NotImplemented(_) => Self::new(StatusCode::NOT_IMPLEMENTED, "not_implemented", error.to_string()),
            StoreError::Crypto => Self::internal("credential vault failure"),
            StoreError::Database(_) | StoreError::Serde(_) => {
                tracing::error!(%error, "store failure");
                Self::internal("storage failure")
            }
        }
    }
}

impl From<RuntimeError> for ApiError {
    fn from(error: RuntimeError) -> Self {
        match error {
            RuntimeError::UnknownAgent(_) => Self::not_found(error.to_string()),
            RuntimeError::AlreadyRunning(_) => Self::conflict("conflict", error.to_string()),
        }
    }
}

impl From<SpellError> for ApiError {
    fn from(error: SpellError) -> Self {
        match error {
            SpellError::Validation(errors) => Self::bad_request("invalid_spell", errors.iter().map(ToString::to_string).collect::<Vec<_>>().join("; ")),
            SpellError::NotImplemented(what) => Self::new(StatusCode::NOT_IMPLEMENTED, "not_implemented", format!("not implemented yet: {what}")),
            SpellError::Timeout => Self::new(StatusCode::GATEWAY_TIMEOUT, "timeout", error.to_string()),
            SpellError::Cancelled => Self::conflict("cancelled", error.to_string()),
            other => Self::new(StatusCode::UNPROCESSABLE_ENTITY, "spell_failed", other.to_string()),
        }
    }
}

impl From<JsonRejection> for ApiError {
    fn from(rejection: JsonRejection) -> Self {
        match rejection.status() {
            StatusCode::UNSUPPORTED_MEDIA_TYPE => Self::new(StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported_media_type", rejection.body_text()),
            _ => Self::bad_request("invalid_body", rejection.body_text()),
        }
    }
}

impl From<PathRejection> for ApiError {
    fn from(rejection: PathRejection) -> Self {
        Self::bad_request("invalid_path", rejection.body_text())
    }
}

impl From<QueryRejection> for ApiError {
    fn from(rejection: QueryRejection) -> Self {
        Self::bad_request("invalid_query", rejection.body_text())
    }
}

#[derive(FromRequest)]
#[from_request(via(axum::Json), rejection(ApiError))]
pub struct ApiJson<T>(pub T);

#[derive(FromRequestParts)]
#[from_request(via(axum::extract::Path), rejection(ApiError))]
pub struct ApiPath<T>(pub T);

#[derive(FromRequestParts)]
#[from_request(via(axum::extract::Query), rejection(ApiError))]
pub struct ApiQuery<T>(pub T);

pub type ApiResult<T> = Result<T, ApiError>;
