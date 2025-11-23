use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::info;

use crate::{config::SlackConfig, ClosestIssue, IssueData};

#[derive(Debug, Error)]
pub enum SlackError {
    #[error("http client error: {0}")]
    HttpClient(#[from] reqwest::Error),
    #[error("invalid auth token value: {0}")]
    InvalidHeader(#[from] reqwest::header::InvalidHeaderValue),
}

#[derive(Deserialize)]
struct PostMessageResponse {
    ts: String,
}

#[derive(Serialize)]
struct SlackBody {
    channel: String,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    thread_ts: Option<String>,
}

impl SlackBody {
    pub fn new(channel: &str, text: String, thread_ts: Option<String>) -> Self {
        Self {
            channel: channel.to_owned(),
            text,
            thread_ts,
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
        summary: String,
        issue: &IssueData,
        closest_issues: &[ClosestIssue],
    ) -> Result<(), SlackError> {
        let mut msg = vec![format!(
            "Closest issues for <{}|#{}>:\n{}\n",
            issue.html_url, issue.number, summary
        )];
        for ci in closest_issues {
            msg.push(format!("• {} (<{}|#{}>)", ci.title, ci.html_url, ci.number));
        }
        let body = SlackBody::new(&self.channel, msg.join("\n"), None);
        let res: PostMessageResponse = self
            .client
            .post(&self.chat_write_url)
            .json(&body)
            .send()
            .await?
            .json()
            .await?;
        let body = SlackBody::new(
            &self.channel,
            format!("*{}*\n---\n{}", issue.title, issue.body),
            Some(res.ts),
        );
        self.client
            .post(&self.chat_write_url)
            .json(&body)
            .send()
            .await?;
        info!("sent closest issues to slack channel:\n{}", body.text);
        Ok(())
    }
}
