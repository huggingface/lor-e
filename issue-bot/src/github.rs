use std::time::Duration;

use async_stream::try_stream;
use chrono::Utc;
use futures::Stream;
use reqwest::{
    header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, AUTHORIZATION, LINK},
    Client,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::time::sleep;
use tracing::info;

use crate::{
    config::{GithubApiConfig, MessageConfig},
    deserialize_null_default, ClosestIssue, RepositoryData, APP_USER_AGENT,
};

const X_RATELIMIT_REMAINING: HeaderName = HeaderName::from_static("x-ratelimit-remaining");
const X_RATELIMIT_RESET: HeaderName = HeaderName::from_static("x-ratelimit-reset");

#[derive(Debug, Error)]
pub enum GithubApiError {
    #[error("invalid header value: {0}")]
    InvalidHeaderValue(#[from] reqwest::header::InvalidHeaderValue),
    #[error("missing rate limit headers: {0:?} {1:?}")]
    MissingRateLimitHeaders(Option<HeaderValue>, Option<HeaderValue>),
    #[error("parse int error: {0}")]
    ParseInt(#[from] std::num::ParseIntError),
    #[error("reqwest error: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("semaphore acquire error: {0}")]
    SemaphoreAcquire(#[from] tokio::sync::AcquireError),
    #[error("serde_json error: {0}")]
    SerdeJson(#[from] serde_json::Error),
    #[error("tokio task join error: {0}")]
    TaskJoin(#[from] tokio::task::JoinError),
    #[error("to str error: {0}")]
    ToStr(#[from] axum::http::header::ToStrError),
}

#[derive(Debug, Deserialize)]
#[allow(unused)]
struct PullRequest {
    html_url: String,
    url: String,
}

#[derive(Debug, Deserialize)]
struct Issue {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    body: String,
    comments_url: String,
    html_url: String,
    id: i64,
    number: i32,
    #[serde(default)]
    pull_request: Option<PullRequest>,
    title: String,
    url: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Comment {
    pub(crate) body: String,
    pub(crate) id: i64,
    pub(crate) url: String,
}

#[derive(Debug)]
pub(crate) struct IssueWithComments {
    pub(crate) body: String,
    pub(crate) comments: Vec<Comment>,
    pub(crate) html_url: String,
    pub(crate) id: i64,
    pub(crate) is_pull_request: bool,
    pub(crate) number: i32,
    pub(crate) title: String,
    pub(crate) url: String,
}

impl IssueWithComments {
    fn new(issue: Issue, comments: Vec<Comment>) -> Self {
        IssueWithComments {
            body: issue.body,
            comments,
            html_url: issue.html_url,
            id: issue.id,
            is_pull_request: issue.pull_request.is_some(),
            number: issue.number,
            title: issue.title,
            url: issue.url,
        }
    }
}

#[derive(Serialize)]
struct CommentBody {
    body: String,
}

#[derive(Clone)]
pub struct GithubApi {
    client: Client,
    comments_enabled: bool,
    message_config: MessageConfig,
}

fn get_next_page(link_header: Option<HeaderValue>) -> Result<Option<String>, GithubApiError> {
    let header = match link_header {
        Some(h) => h.to_str()?.to_owned(),
        None => return Ok(None),
    };

    Ok(header
        .split(", ")
        .find(|part| part.contains("rel=\"next\""))
        .map(|part| {
            part.chars()
                .skip(1)
                .take_while(|c| *c != '>')
                .collect::<String>()
        }))
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
            comments_enabled: cfg.comments_enabled,
            message_config,
        })
    }

    pub async fn comment_on_issue(
        &self,
        issue_url: &str,
        closest_issues: Vec<ClosestIssue>,
    ) -> Result<(), GithubApiError> {
        if !self.comments_enabled {
            return Ok(());
        }

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

    pub(crate) fn get_issues(
        &self,
        from_page: i32,
        repo_data: RepositoryData,
    ) -> impl Stream<Item = Result<(IssueWithComments, Option<i32>), GithubApiError>> + use<'_>
    {
        try_stream! {
            let url = format!("https://api.github.com/repos/{}/issues", repo_data.full_name);
            let client = self.client.clone();
            let mut page = from_page;
            loop {
                let res = client
                    .get(&url)
                    .query(&[
                        ("state", "all"),
                        ("direction", "desc"),
                        ("page", &page.to_string()),
                        ("per_page", "100"),
                    ])
                .send()
                .await?;
                let link_header = res.headers().get(LINK).cloned();
                let ratelimit_remaining = res.headers().get(X_RATELIMIT_REMAINING).cloned();
                let ratelimit_reset = res.headers().get(X_RATELIMIT_RESET).cloned();
                let issues = res.json::<Vec<Issue>>().await?;
                info!("fetched {} issues from page {}, getting comments for each issue next", issues.len(), page);
                handle_ratelimit(ratelimit_remaining, ratelimit_reset).await?;
                let page_issue_count = issues.len();
                for (i, issue) in issues.into_iter().enumerate() {
                    let res = client
                        .get(&issue.comments_url)
                        .query(&[("direction", "asc")])
                        .send()
                        .await?;
                    let ratelimit_remaining = res.headers().get(X_RATELIMIT_REMAINING).cloned();
                    let ratelimit_reset = res.headers().get(X_RATELIMIT_RESET).cloned();
                    handle_ratelimit(ratelimit_remaining, ratelimit_reset).await?;
                    let comments = res
                        .json::<Vec<Comment>>()
                        .await?;
                    yield (IssueWithComments::new(issue, comments), (i + 1 == page_issue_count).then_some(page));
                }
                if get_next_page(link_header)?.is_none() {
                    break;
                }
                page += 1;
            }
        }
    }
}

async fn handle_ratelimit(
    remaining: Option<HeaderValue>,
    reset: Option<HeaderValue>,
) -> Result<(), GithubApiError> {
    match (remaining, reset) {
        (Some(remaining), Some(reset)) => {
            let remaining: i32 = remaining.to_str()?.parse()?;
            let reset: i64 = reset.to_str()?.parse()?;
            if remaining == 0 {
                let duration = Duration::from_secs((reset - Utc::now().timestamp() + 2) as u64);
                info!("rate limit reached, sleeping for {}s", duration.as_secs());
                sleep(duration).await;
            }
        }
        (remaining, reset) => {
            return Err(GithubApiError::MissingRateLimitHeaders(remaining, reset))
        }
    }
    Ok(())
}
