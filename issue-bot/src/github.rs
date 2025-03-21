use reqwest::{
    header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION},
    Client,
};
use serde::Serialize;
use thiserror::Error;

use crate::{
    config::{GithubApiConfig, MessageConfig},
    ClosestIssue,
};

static APP_USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"),);

#[derive(Debug, Error)]
pub enum GithubApiError {
    #[error("invalid header value: {0}")]
    InvalidHeaderValue(#[from] reqwest::header::InvalidHeaderValue),
    #[error("reqwest error: {0}")]
    Reqwest(#[from] reqwest::Error),
}

#[derive(Serialize)]
struct CommentBody {
    body: String,
}

pub struct GithubApi {
    client: Client,
    message_config: MessageConfig,
}

impl GithubApi {
    pub fn new(
        cfg: GithubApiConfig,
        message_config: MessageConfig,
    ) -> Result<Self, GithubApiError> {
        let mut headers = HeaderMap::new();
        let mut auth_value = HeaderValue::from_str(&format!("Bearer {}", cfg.auth_token))?;
        auth_value.set_sensitive(true);
        headers.insert(AUTHORIZATION, auth_value);
        headers.insert(
            ACCEPT,
            HeaderValue::from_str("application/vnd.github+json")?,
        );
        headers.insert("X-GitHub-Api-Version", HeaderValue::from_str("2022-11-28")?);
        let client = Client::builder()
            .user_agent(APP_USER_AGENT)
            .default_headers(headers)
            .build()?;

        Ok(Self {
            client,
            message_config,
        })
    }

    pub async fn comment_on_issue(
        &self,
        issue_url: &str,
        closest_issues: Vec<ClosestIssue>,
    ) -> Result<(), GithubApiError> {
        let comment_url = format!("{issue_url}/comments");
        let issues: Vec<String> = closest_issues
            .into_iter()
            .map(|i| format!("- {} ([#{}]({}))", i.title, i.number, i.html_url))
            .collect();
        let body = format!(
            "{}{}{}",
            self.message_config.pre,
            issues.join("\n"),
            self.message_config.post
        );
        self.client
            .post(comment_url)
            .json(&CommentBody { body })
            .send()
            .await?;
        Ok(())
    }
}
