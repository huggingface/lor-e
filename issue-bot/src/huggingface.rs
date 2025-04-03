use reqwest::{
    header::{HeaderMap, HeaderValue, AUTHORIZATION},
    Client,
};
use serde::Serialize;
use thiserror::Error;

use crate::{
    config::{HuggingfaceApiConfig, MessageConfig},
    ClosestIssue,
};

static APP_USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"),);

#[derive(Debug, Error)]
pub enum HuggingfaceApiError {
    #[error("invalid header value: {0}")]
    InvalidHeaderValue(#[from] reqwest::header::InvalidHeaderValue),
    #[error("reqwest error: {0}")]
    Reqwest(#[from] reqwest::Error),
}

#[derive(Serialize)]
struct CommentBody {
    comment: String,
}

pub struct HuggingfaceApi {
    client: Client,
    comments_enabled: bool,
    message_config: MessageConfig,
}

impl HuggingfaceApi {
    pub fn new(
        cfg: HuggingfaceApiConfig,
        message_config: MessageConfig,
    ) -> Result<Self, HuggingfaceApiError> {
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
            comments_enabled: cfg.comments_enabled,
            message_config,
        })
    }

    pub async fn comment_on_issue(
        &self,
        issue_url: &str,
        closest_issues: Vec<ClosestIssue>,
    ) -> Result<(), HuggingfaceApiError> {
        if !self.comments_enabled {
            return Ok(());
        }

        let comment_url = format!("{issue_url}/comment");
        let issues: Vec<String> = closest_issues
            .into_iter()
            .map(|i| format!("- {} ([#{}]({}))", i.title, i.number, i.html_url))
            .collect();
        let comment = format!(
            "{}{}{}",
            self.message_config.pre,
            issues.join("\n"),
            self.message_config.post
        );
        self.client
            .post(comment_url)
            .json(&CommentBody { comment })
            .send()
            .await?;
        Ok(())
    }
}
