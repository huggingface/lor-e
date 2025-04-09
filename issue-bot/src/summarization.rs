use reqwest::{
    header::{HeaderMap, HeaderValue, AUTHORIZATION},
    Client,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{config::SummarizationApiConfig, APP_USER_AGENT};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Message {
    content: String,
    role: String,
}

#[derive(Serialize)]
pub struct ChatCompletionsRequest {
    max_tokens: u32,
    messages: Vec<Message>,
    model: String,
    stream: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ChatCompletionsChoice {
    message: Message,
}

#[derive(Debug, Deserialize)]
pub struct ChatCompletionsResponse {
    choices: Vec<ChatCompletionsChoice>,
}

#[derive(Debug, Error)]
pub enum SummarizationApiError {
    #[error("invalid header value: {0}")]
    InvalidHeaderValue(#[from] reqwest::header::InvalidHeaderValue),
    #[error("reqwest error: {0}")]
    Reqwest(#[from] reqwest::Error),
}

pub struct SummarizationApi {
    client: Client,
    model: String,
    special_tokens: Vec<String>,
    system_prompt: String,
    url: String,
}

impl SummarizationApi {
    pub fn new(cfg: SummarizationApiConfig) -> Result<Self, SummarizationApiError> {
        let mut headers = HeaderMap::new();
        let mut auth_value = HeaderValue::from_str(&format!("Bearer {}", cfg.auth_token))?;
        auth_value.set_sensitive(true);
        headers.insert(AUTHORIZATION, auth_value);
        let client = Client::builder()
            .user_agent(APP_USER_AGENT)
            .default_headers(headers)
            .build()?;
        Ok(Self {
            client,
            model: cfg.model,
            special_tokens: cfg.special_tokens_used,
            system_prompt: cfg.system_prompt,
            url: cfg.url,
        })
    }

    pub async fn summarize(&self, text: String) -> Result<String, SummarizationApiError> {
        let chat_completions_url = format!("{}/v1/chat/completions", self.url);
        let res: ChatCompletionsResponse = self
            .client
            .post(chat_completions_url)
            .json(&ChatCompletionsRequest {
                max_tokens: 100,
                messages: vec![
                    Message {
                        role: "system".to_owned(),
                        content: self.system_prompt.clone(),
                    },
                    Message {
                        role: "user".to_owned(),
                        content: text,
                    },
                ],
                model: self.model.to_owned(),
                stream: false,
            })
            .send()
            .await?
            .json()
            .await?;
        let mut res = res
            .choices
            .first()
            .cloned()
            .map(|c| c.message.content)
            .unwrap_or_default();
        for token in self.special_tokens.iter() {
            res = res.replace(&format!("<{token}>"), "");
            res = res.replace(&format!("</{token}>"), "");
        }
        Ok(res)
    }
}
