use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use serde::Serialize;
use thiserror::Error;

use crate::{config::SlackConfig, ClosestIssue};

#[derive(Debug, Error)]
pub enum SlackError {
    #[error("http client error: {0}")]
    HttpClient(#[from] reqwest::Error),
    #[error("invalid auth token value: {0}")]
    InvalidHeader(#[from] reqwest::header::InvalidHeaderValue),
}

#[derive(Serialize)]
struct SlackBody {
    channel: String,
    text: String,
}

impl SlackBody {
    pub fn new(channel: &str, text: String) -> Self {
        Self {
            channel: channel.to_owned(),
            text,
        }
    }
}

#[derive(Clone)]
pub struct Slack {
    channel: String,
    chat_write_url: String,
    client: reqwest::Client,
}

impl Slack {
    pub fn new(config: &SlackConfig) -> Result<Self, SlackError> {
        let mut headers = HeaderMap::new();

        let mut auth_value = HeaderValue::from_str(&format!("Bearer {}", config.auth_token))?;
        auth_value.set_sensitive(true);
        headers.insert(AUTHORIZATION, auth_value);

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()?;

        Ok(Self {
            channel: config.channel.to_owned(),
            chat_write_url: config.chat_write_url.to_owned(),
            client,
        })
    }

    pub async fn closest_issues(
        &self,
        issue_title: &str,
        issue_number: i32,
        issue_html_url: &str,
        closest_issues: &[ClosestIssue],
    ) -> Result<(), SlackError> {
        let mut msg = vec![format!(
            "Closest issues for {} <{}|#{}>",
            issue_title, issue_number, issue_html_url
        )];
        for ci in closest_issues {
            msg.push(format!("- {} (<{}|#{}>)", ci.title, ci.html_url, ci.number));
        }
        let body = SlackBody::new(&self.channel, msg.join("\n"));
        self.client
            .post(self.chat_write_url.to_owned())
            .json(&body)
            .send()
            .await?;
        Ok(())
    }
}
