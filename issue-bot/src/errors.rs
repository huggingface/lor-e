use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use thiserror::Error;
use tracing::error;

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("auth error")]
    Auth,
    #[error("auth error")]
    Axum(#[from] axum::Error),
    #[error("embedding error: {0}")]
    Embedding(#[from] crate::embeddings::EmbeddingError),
    #[error("hmac key invalid length")]
    Hmac(#[from] hmac::digest::InvalidLength),
    #[error("serde json error: {0}")]
    SerdeJson(#[from] serde_json::Error),
    #[error("signatures don't match")]
    SignatureMismatch,
    #[error("sqlx error: {0}")]
    Sqlx(#[from] sqlx::error::Error),
    #[error("to str error: {0}")]
    ToStr(#[from] axum::http::header::ToStrError),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, error_message) = match self {
            ApiError::Auth => (
                StatusCode::UNAUTHORIZED,
                StatusCode::UNAUTHORIZED.to_string(),
            ),
            ApiError::Axum(err) => {
                error!("{}", err);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Internal server error".to_string(),
                )
            }
            ApiError::Embedding(err) => {
                error!("{}", err);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Internal server error".to_string(),
                )
            }
            ApiError::Hmac(err) => {
                error!("{}", err);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Internal server error".to_string(),
                )
            }
            ApiError::SerdeJson(err) => {
                error!("{}", err);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Internal server error".to_string(),
                )
            }
            ApiError::SignatureMismatch => {
                (StatusCode::FORBIDDEN, StatusCode::FORBIDDEN.to_string())
            }
            ApiError::Sqlx(err) => {
                error!("{}", err);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Internal server error".to_string(),
                )
            }
            ApiError::ToStr(err) => {
                error!("{}", err);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Internal server error".to_string(),
                )
            }
        };

        let body = Json(json!({
            "error": error_message,
        }));

        (status, body).into_response()
    }
}
