use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use campaign_client::ClientError;
use campaign_types::combat::CombatError;
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("combat not found")]
    NotFound,
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Forbidden(String),
    /// The character's sheet refused what the command costs (no slot left).
    #[error("{0}")]
    Sheet(String),
    /// The character service couldn't be reached to spend it.
    #[error("couldn't reach the character's sheet: {0}")]
    SheetUnavailable(String),
    #[error(transparent)]
    Combat(#[from] CombatError),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("corrupt stored data: {0}")]
    Corrupt(String),
}

impl ApiError {
    /// What the character service said about spending on a sheet.
    pub fn from_sheet(error: ClientError) -> Self {
        match error {
            ClientError::Exhausted(message) | ClientError::Rejected(message) => {
                ApiError::Sheet(message)
            }
            ClientError::NotFound => ApiError::Sheet("the character's sheet is gone".into()),
            other => ApiError::SheetUnavailable(other.to_string()),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self {
            ApiError::NotFound => StatusCode::NOT_FOUND,
            ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::Forbidden(_) => StatusCode::FORBIDDEN,
            ApiError::Sheet(_) => StatusCode::UNPROCESSABLE_ENTITY,
            ApiError::SheetUnavailable(_) => StatusCode::BAD_GATEWAY,
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
