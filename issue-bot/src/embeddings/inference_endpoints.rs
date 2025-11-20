use std::time::Duration;

use reqwest::{
    header::{HeaderMap, HeaderValue, AUTHORIZATION},
    Client,
};
use serde::Serialize;
use tracing::warn;

use crate::{config::EmbeddingApiConfig, APP_USER_AGENT};

use super::EmbeddingError;

#[derive(Serialize)]
enum TruncateDirection {
    #[allow(unused)]
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
    cfg: EmbeddingApiConfig,
    client: Client,
}

impl EmbeddingApi {
    pub fn new(cfg: EmbeddingApiConfig) -> Result<Self, EmbeddingError> {
        let mut headers = HeaderMap::new();
        let mut auth_value = HeaderValue::from_str(&format!("Bearer {}", cfg.auth_token))?;
        auth_value.set_sensitive(true);
        headers.insert(AUTHORIZATION, auth_value);
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(APP_USER_AGENT)
            .default_headers(headers)
            .build()?;

        Ok(Self { cfg, client })
    }

    pub async fn generate_embedding(&self, text: String) -> Result<Vec<f32>, EmbeddingError> {
        let max_retries = 5;
        let mut retries = 0;
        loop {
            let res = self
                .client
                .post(&self.cfg.url)
                .json(&EmbedRequest {
                    inputs: text.clone(),
                    truncate: true,
                    truncate_direction: TruncateDirection::Right,
                })
                .send()
                .await;
            let res = match res {
                Err(e) => {
                    if e.is_timeout() {
                        warn!("Embedding API request timed out");
                        retries += 1;
                        if retries > max_retries {
                            return Err(EmbeddingError::MaxRetriesExceeded(max_retries));
                        }
                        tokio::time::sleep(Duration::from_secs(2_u64.pow(retries))).await;
                        continue;
                    }
                    return Err(e.into());
                }
                Ok(res) => res,
            };
            if res.status() != 200 {
                let status = res.status();
                let response_content = res.text().await?;
                warn!(
                    "[status: {}] Embedding API returned: '{}'",
                    status, response_content
                );
                retries += 1;
                if retries > max_retries {
                    return Err(EmbeddingError::MaxRetriesExceeded(max_retries));
                }
                tokio::time::sleep(Duration::from_secs(2_u64.pow(retries))).await;
                continue;
            }
            return res
                .json::<Vec<Vec<f32>>>()
                .await?
                .pop()
                .ok_or(EmbeddingError::MissingEmbedding);
        }
    }
}
