use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use campaign_types::{monster::MonsterError, rules_text::RulesTextError};

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("not found")]
    NotFound,
    /// The shared library is changed by importing, not through the API.
    #[error("the shared library can't be changed here; make a copy for the campaign instead")]
    Shared,
    /// Only the DM's devices change the campaign's monsters, spells and
    /// conditions.
    #[error("only a DM device can change the campaign's monsters, spells and conditions")]
    DmOnly,
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
            ApiError::Shared | ApiError::DmOnly => StatusCode::FORBIDDEN,
            ApiError::Invalid(_) | ApiError::InvalidText(_) => StatusCode::UNPROCESSABLE_ENTITY,
            ApiError::Database(_) | ApiError::Corrupt(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        campaign_server::error_response(status, &self)
    }
}
