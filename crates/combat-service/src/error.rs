use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use campaign_types::combat::CombatError;
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("combat not found")]
    NotFound,
    #[error("{0}")]
    BadRequest(String),
    #[error(transparent)]
    Combat(#[from] CombatError),
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
            ApiError::Combat(_) => StatusCode::UNPROCESSABLE_ENTITY,
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
