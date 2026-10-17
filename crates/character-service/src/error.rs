use axum::{
    Json,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use campaign_types::{ActionError, Character, ValidationError};
use serde_json::json;

use crate::routes::etag;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("character not found")]
    NotFound,
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Forbidden(String),
    #[error("an If-Match header with the character's current ETag is required")]
    PreconditionRequired,
    /// The client's copy is stale. Carries the current character so the
    /// client can re-apply its change without another round trip.
    #[error("character has been modified (current version is {})", .0.version)]
    VersionMismatch(Box<Character>),
    #[error(transparent)]
    Validation(#[from] ValidationError),
    #[error(transparent)]
    Action(#[from] ActionError),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("corrupt stored data: {0}")]
    Corrupt(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self {
            ApiError::NotFound => StatusCode::NOT_FOUND,
            ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::Forbidden(_) => StatusCode::FORBIDDEN,
            ApiError::PreconditionRequired => StatusCode::PRECONDITION_REQUIRED,
            ApiError::VersionMismatch(_) => StatusCode::PRECONDITION_FAILED,
            ApiError::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
            // Out of slots/uses is a conflict with the character's current
            // state; anything else means the request itself was wrong.
            ApiError::Action(e) if e.is_exhausted() => StatusCode::CONFLICT,
            ApiError::Action(_) => StatusCode::UNPROCESSABLE_ENTITY,
            ApiError::Database(_) | ApiError::Corrupt(_) => {
                tracing::error!(error = %self, "internal error");
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };
        let message = match status {
            StatusCode::INTERNAL_SERVER_ERROR => "internal server error".to_string(),
            _ => self.to_string(),
        };
        match self {
            ApiError::VersionMismatch(current) => (
                status,
                [(header::ETAG, etag(&current))],
                Json(json!({ "error": message, "current": current })),
            )
                .into_response(),
            _ => (status, Json(json!({ "error": message }))).into_response(),
        }
    }
}
