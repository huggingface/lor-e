use reqwest::{
    header::{HeaderMap, HeaderValue, AUTHORIZATION},
    Client,
};
use serde::Serialize;

use crate::config::ModelApiConfig;

use super::EmbeddingError;

#[derive(Serialize)]
enum TruncateDirection {
    Left,
    Right,
}

#[derive(Serialize)]
struct EmbedRequest {
    inputs: String,
    truncate: bool,
    truncate_direction: TruncateDirection,
}

#[derive(Clone)]
pub struct EmbeddingApi {
    cfg: ModelApiConfig,
    client: Client,
}

impl EmbeddingApi {
    pub async fn new(cfg: ModelApiConfig) -> Result<Self, EmbeddingError> {
        let mut headers = HeaderMap::new();
        let mut auth_value = HeaderValue::from_str(&format!("Bearer {}", cfg.auth_token))?;
        auth_value.set_sensitive(true);
        headers.insert(AUTHORIZATION, auth_value);
        let client = Client::builder().default_headers(headers).build()?;

        Ok(Self { cfg, client })
    }

    // TODO: handle API errors gracefully
    pub async fn generate_embedding(&self, text: String) -> Result<Vec<f32>, EmbeddingError> {
        self.client
            .post(&self.cfg.url)
            .json(&EmbedRequest {
                inputs: text,
                truncate: true,
                truncate_direction: TruncateDirection::Right,
            })
            .send()
            .await?
            .json::<Vec<Vec<f32>>>()
            .await?
            .pop()
            .ok_or(EmbeddingError::MissingEmbedding)
    }
}
