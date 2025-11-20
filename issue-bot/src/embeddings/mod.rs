use reqwest::StatusCode;
use thiserror::Error;

pub mod inference_endpoints;
// mod local;

#[derive(Debug, Error)]
pub enum EmbeddingError {
    // #[error("candle error: {0}")]
    // Candle(#[from] candle::Error),
    // #[error("hf hub error: {0}")]
    // HfHub(#[from] hf_hub::api::tokio::ApiError),
    #[error("http client error: {0}")]
    HttpClientError(StatusCode),
    #[error("invalid header value: {0}")]
    InvalidHeaderValue(#[from] reqwest::header::InvalidHeaderValue),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("join error: {0}")]
    Join(#[from] tokio::task::JoinError),
    #[error("maximum retries ({0}) exceeded")]
    MaxRetriesExceeded(u32),
    #[error("no embedding was returned from the API")]
    MissingEmbedding,
    #[error("reqwest error: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("serde json error: {0}")]
    SerdeJson(#[from] serde_json::Error),
    // #[error("tokenizers error: {0}")]
    // Tokenizers(#[from] tokenizers::Error),
}
