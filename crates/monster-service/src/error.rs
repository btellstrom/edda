use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use edda_types::{monster::MonsterError, rules_text::RulesTextError};

/// What a request can fail with, and the status it answers.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("not found")]
    NotFound,
    /// The shared library is changed by importing, not through the API.
    #[error("the shared library can't be changed here; make a copy for the campaign instead")]
    Shared,
    /// The campaign keeps as many as it may.
    #[error("{0}")]
    Limit(String),
    /// Only the DM's devices change the campaign's monsters, spells and
    /// conditions.
    #[error("only a DM device can change the campaign's monsters, spells and conditions")]
    DmOnly,
    /// Not what was asked for, as an export of another campaign.
    #[error("{0}")]
    BadRequest(String),
    /// Something's in the way, as a campaign's own of an import.
    #[error("{0}")]
    Conflict(String),
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
            ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::Conflict(_) => StatusCode::CONFLICT,
            ApiError::Shared | ApiError::DmOnly | ApiError::Limit(_) => StatusCode::FORBIDDEN,
            ApiError::Invalid(_) | ApiError::InvalidText(_) => StatusCode::UNPROCESSABLE_ENTITY,
            ApiError::Database(_) | ApiError::Corrupt(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        edda_server::error_response(status, &self)
    }
}
