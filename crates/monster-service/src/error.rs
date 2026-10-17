use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use campaign_types::{monster::MonsterError, rules_text::RulesTextError};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("not found")]
    NotFound,
    /// The shared library is changed by importing, not through the API.
    #[error("the shared library can't be changed here; make a copy for the campaign instead")]
    Shared,
    #[error(transparent)]
    Invalid(#[from] MonsterError),
    #[error(transparent)]
    InvalidText(#[from] RulesTextError),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("corrupt stored data: {0}")]
    Corrupt(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self {
            ApiError::NotFound => StatusCode::NOT_FOUND,
            ApiError::Shared => StatusCode::FORBIDDEN,
            ApiError::Invalid(_) | ApiError::InvalidText(_) => StatusCode::UNPROCESSABLE_ENTITY,
            ApiError::Database(_) | ApiError::Corrupt(_) => {
                tracing::error!(error = %self, "internal error");
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };
        let message = match status {
            StatusCode::INTERNAL_SERVER_ERROR => "internal server error".to_string(),
            _ => self.to_string(),
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}
