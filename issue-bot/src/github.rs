use std::sync::Arc;

use futures::stream::FuturesUnordered;
use reqwest::{
    header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, LINK},
    Client,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::{
    sync::{
        mpsc::{self, UnboundedSender},
        Semaphore,
    },
    task::JoinHandle,
};
use tracing::info;

use crate::{
    config::{GithubApiConfig, MessageConfig},
    ClosestIssue, RepositoryData,
};

static APP_USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"),);

#[derive(Debug, Error)]
pub enum GithubApiError {
    #[error("invalid header value: {0}")]
    InvalidHeaderValue(#[from] reqwest::header::InvalidHeaderValue),
    #[error("reqwest error: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("semaphore acquire error: {0}")]
    SemaphoreAcquire(#[from] tokio::sync::AcquireError),
    #[error("failed to send message to channel")]
    Send,
    #[error("tokio task join error: {0}")]
    TaskJoin(#[from] tokio::task::JoinError),
    #[error("to str error: {0}")]
    ToStr(#[from] axum::http::header::ToStrError),
}

#[derive(Debug, Deserialize)]
struct PullRequest {
    url: String,
}

#[derive(Debug, Deserialize)]
struct Issue {
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
    id: i64,
    url: String,
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

    pub(crate) async fn get_issues(
        &self,
        issues_tx: UnboundedSender<IssueWithComments>,
        repository: RepositoryData,
    ) -> Result<(), GithubApiError> {
        let mut url = format!("https://api.github.com/repos/{}/issues", repository.repo_id);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let client = self.client.clone();
        let handle = tokio::spawn(async move {
            loop {
                let res = client
                    .get(&url)
                    .query(&[("state", "all"), ("direction", "asc"), ("per_page", "100")])
                    .send()
                    .await?;
                let link_header = res.headers().get(LINK).cloned();
                let issues = res.json::<Vec<Issue>>().await?;
                if let Some(next_page_url) = get_next_page(link_header)? {
                    url = next_page_url;
                } else {
                    break;
                }
                tx.send(issues).map_err(|_| GithubApiError::Send)?;
            }
            Ok(())
        });

        let semaphore = Arc::new(Semaphore::new(16));
        let client = self.client.clone();
        let for_each_handle = tokio::spawn(async move {
            let handles = FuturesUnordered::new();
            while let Some(issues) = rx.recv().await {
                for issue in issues.into_iter().filter(|i| i.pull_request.is_none()) {
                    let client = client.clone();
                    let issues_tx = issues_tx.clone();
                    let permit = semaphore.clone().acquire_owned().await?;
                    handles.push(tokio::spawn(async move {
                        let comments = client
                            .get(&issue.comments_url)
                            .query(&[("direction", "asc")])
                            .send()
                            .await?
                            .json::<Vec<Comment>>()
                            .await?;
                        drop(permit);
                        issues_tx
                            .send(IssueWithComments::new(issue, comments))
                            .map_err(|_| GithubApiError::Send)?;
                        Ok(())
                    }));
                }
            }
            let results: Vec<Result<Result<(), GithubApiError>, tokio::task::JoinError>> =
                futures::future::join_all(handles).await;
            let results: Result<Vec<_>, GithubApiError> = results.into_iter().flatten().collect();
            results?;
            Ok(())
        });

        tokio::try_join!(flatten(handle), flatten(for_each_handle))?;
        Ok(())
    }

    pub(crate) async fn get_prs(
        &self,
        repository: &RepositoryData,
    ) -> Result<Vec<IssueWithComments>, GithubApiError> {
        let issues = Vec::new();
        Ok(issues)
    }
}

async fn flatten(handle: JoinHandle<Result<(), GithubApiError>>) -> Result<(), GithubApiError> {
    match handle.await {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(err)) => Err(err),
        Err(err) => Err(err.into()),
    }
}
