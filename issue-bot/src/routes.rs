use std::{fmt::Display, sync::atomic::Ordering};

use axum::{
    body::Body,
    extract::{FromRef, FromRequestParts, Request, State},
    http::{request::Parts, HeaderName, StatusCode},
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use hmac::{Hmac, Mac};
use reqwest::header::AUTHORIZATION;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tracing::info;

use crate::{
    deserialize_null_default, errors::ApiError, Action, AppState, EventData, RepositoryData,
    Source, PRE_SHUTDOWN,
};

fn compute_signature(payload: &[u8], secret: &str) -> String {
    let key = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    let mut mac = key;
    mac.update(payload);
    let result = mac.finalize().into_bytes();
    format!("sha256={}", hex::encode(result))
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum CommentActionType {
    Created,
    Deleted,
    Edited,
}

impl CommentActionType {
    fn to_action(&self) -> Action {
        match self {
            Self::Created => Action::Created,
            Self::Edited => Action::Edited,
            Self::Deleted => Action::Deleted,
        }
    }
}

impl Display for CommentActionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.serialize(f)
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum IssueActionType {
    Opened,
    Edited,
    Deleted,
    /// We don't care about other action types
    #[serde(other)]
    Ignored,
}
impl IssueActionType {
    fn to_action(&self) -> Action {
        match self {
            Self::Opened => Action::Created,
            Self::Edited => Action::Edited,
            Self::Deleted => Action::Deleted,
            Self::Ignored => unreachable!("IssueActionType::to_action called with Ignored"),
        }
    }
}

impl Display for IssueActionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.serialize(f)
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct Comment {
    body: String,
    id: i64,
    url: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct PullRequest {
    html_url: String,
    url: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct Issue {
    action: IssueActionType,
    issue: IssueData,
    repository: Repository,
}

#[derive(Debug, Deserialize, Serialize)]
struct IssueData {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    body: String,
    html_url: String,
    id: i64,
    number: i32,
    #[serde(default)]
    pull_request: Option<PullRequest>,
    title: String,
    url: String,
}

/// Issue & Pull Request comments
#[derive(Debug, Deserialize, Serialize)]
struct IssueComment {
    action: CommentActionType,
    comment: Comment,
    issue: IssueData,
    repository: Repository,
}

#[derive(Debug, Deserialize, Serialize)]
struct Repository {
    full_name: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
enum GithubWebhook {
    IssueComment(IssueComment),
    Issue(Issue),
}

impl Display for GithubWebhook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let webhook_type = match self {
            Self::Issue(_) => "issue",
            Self::IssueComment(_) => "issue comment",
        };
        write!(f, "{}", webhook_type)
    }
}

pub async fn github_webhook(
    State(state): State<AppState>,
    req: Request<Body>,
) -> anyhow::Result<(), ApiError> {
    let header_name = HeaderName::from_static("x-hub-signature-256");
    let sig = req
        .headers()
        .get(header_name)
        .ok_or(ApiError::SignatureMismatch)?
        .clone();
    let body = req.into_body();
    let body_bytes = axum::body::to_bytes(body, usize::MAX).await?;
    let expected_sig = compute_signature(&body_bytes, &state.auth_token);

    if expected_sig != sig {
        return Err(ApiError::SignatureMismatch);
    }

    let webhook = serde_json::from_slice::<GithubWebhook>(&body_bytes)?;
    let ongoing_indexation = state.ongoing_indexation.read().await;
    let webhook_type = webhook.to_string();
    match webhook {
        GithubWebhook::Issue(issue) => {
            let idx_process = ongoing_indexation.get(&issue.repository.full_name);
            if idx_process.is_some() {
                return Err(ApiError::IndexationInProgress);
            }
            info!("received {} (state: {})", webhook_type, issue.action);
            match issue.action {
                IssueActionType::Opened | IssueActionType::Edited | IssueActionType::Deleted => {
                    state
                        .tx
                        .send(EventData::Issue(crate::IssueData {
                            source_id: issue.issue.id.to_string(),
                            action: issue.action.to_action(),
                            title: issue.issue.title,
                            body: issue.issue.body,
                            is_pull_request: issue.issue.pull_request.is_some(),
                            number: issue.issue.number,
                            html_url: issue.issue.html_url,
                            url: issue.issue.url,
                            repository_full_name: issue.repository.full_name,
                            source: Source::Github,
                        }))
                        .await?
                }
                IssueActionType::Ignored => (),
            }
        }
        GithubWebhook::IssueComment(comment) => {
            let idx_process = ongoing_indexation.get(&comment.repository.full_name);
            if idx_process.is_some() {
                return Err(ApiError::IndexationInProgress);
            }
            info!("received {} (state: {})", webhook_type, comment.action);
            state
                .tx
                .send(EventData::Comment(crate::CommentData {
                    source_id: comment.comment.id.to_string(),
                    issue_id: comment.issue.id.to_string(),
                    action: comment.action.to_action(),
                    body: comment.comment.body,
                    url: comment.comment.url,
                }))
                .await?;
        }
    }

    Ok(())
}

const X_WEBHOOK_SECRET: HeaderName = HeaderName::from_static("x-webhook-secret");

pub struct HfWebhookSecretValidator;

impl<S> FromRequestParts<S> for HfWebhookSecretValidator
where
    AppState: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let state = AppState::from_ref(state);
        let secret = parts
            .headers
            .get(X_WEBHOOK_SECRET)
            .cloned()
            .ok_or(ApiError::Auth)?;

        if secret != state.auth_token {
            return Err(ApiError::Auth);
        }

        Ok(Self)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum HfAction {
    Create,
    Update,
    Delete,
}

impl HfAction {
    fn to_action(&self) -> Action {
        match self {
            Self::Create => Action::Created,
            Self::Update => Action::Edited,
            Self::Delete => Action::Deleted,
        }
    }
}

impl Display for HfAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let action = match self {
            Self::Create => "create",
            Self::Update => "update",
            Self::Delete => "delete",
        };
        write!(f, "{}", action)
    }
}

#[derive(Debug, Deserialize)]
enum Scope {
    #[serde(rename = "discussion")]
    Discussion,
    #[serde(rename = "discussion.comment")]
    DiscussionComment,
}

impl Display for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let scope = match self {
            Self::Discussion => "discussion",
            Self::DiscussionComment => "discussion.comment",
        };
        write!(f, "{}", scope)
    }
}

#[derive(Debug, Deserialize)]
struct Event {
    action: HfAction,
    scope: Scope,
}

#[derive(Debug, Deserialize)]
struct Url {
    web: String,
    api: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Discussion {
    id: String,
    is_pull_request: bool,
    num: i32,
    title: String,
    url: Url,
}

#[derive(Debug, Deserialize)]
struct WebUrl {
    web: String,
}

#[derive(Debug, Deserialize)]
struct Author {
    id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HfComment {
    id: String,
    #[serde(default)]
    content: String,
    author: Author,
    url: WebUrl,
}

#[derive(Debug, Deserialize)]
pub struct HuggingfaceWebhook {
    event: Event,
    discussion: Option<Discussion>,
    comment: Option<HfComment>,
}

pub async fn huggingface_webhook(
    HfWebhookSecretValidator: HfWebhookSecretValidator,
    State(state): State<AppState>,
    Json(webhook): Json<HuggingfaceWebhook>,
) -> Result<(), ApiError> {
    info!(
        "received {} (status: {})",
        webhook.event.scope, webhook.event.action
    );

    let discussion = match webhook.discussion {
        Some(discussion) => discussion,
        None => {
            return Err(ApiError::MalformedWebhook(format!(
                r#"Missing discussion when event.scope = "{}" and event.action = "{}""#,
                webhook.event.scope, webhook.event.action
            )))
        }
    };
    match webhook.event.scope {
        Scope::Discussion => {
            let comment_content = match webhook.comment {
                Some(comment) => comment.content,
                None => String::new(),
            };
            state
                .tx
                .send(EventData::Issue(crate::IssueData {
                    source_id: discussion.id,
                    action: webhook.event.action.to_action(),
                    title: discussion.title,
                    body: comment_content,
                    is_pull_request: discussion.is_pull_request,
                    number: discussion.num,
                    html_url: discussion.url.web,
                    url: discussion.url.api,
                    repository_full_name: String::new(), // TODO: extract repository full name from discussion url
                    source: Source::HuggingFace,
                }))
                .await?;
        }
        Scope::DiscussionComment => {
            let comment = match webhook.comment {
                Some(comment) => comment,
                None => {
                    return Err(ApiError::MalformedWebhook(format!(
                        r#"Missing comment when event.scope = "{}" and event.action = "{}""#,
                        webhook.event.scope, webhook.event.action
                    )))
                }
            };
            // NOTE: check if comment is from `lor-e-bot`
            if comment.author.id != "67e0825265e294ad98833748" {
                state
                    .tx
                    .send(EventData::Comment(crate::CommentData {
                        source_id: comment.id,
                        action: webhook.event.action.to_action(),
                        body: comment.content,
                        issue_id: discussion.id,
                        url: comment.url.web,
                    }))
                    .await?;
            }
        }
    }
    Ok(())
}

pub fn event_router() -> Router<AppState> {
    Router::new()
        .route("/github", post(github_webhook))
        .route("/huggingface", post(huggingface_webhook))
}

pub struct SecretValidator;

impl<S> FromRequestParts<S> for SecretValidator
where
    AppState: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let state = AppState::from_ref(state);
        let secret = parts
            .headers
            .get(AUTHORIZATION)
            .cloned()
            .ok_or(ApiError::Auth)?;

        if secret != state.auth_token {
            return Err(ApiError::Auth);
        }

        Ok(Self)
    }
}

// TODO: reply id and endpoint to query progress?
pub async fn index_repository(
    SecretValidator: SecretValidator,
    State(state): State<AppState>,
    Json(repo_data): Json<RepositoryData>,
) -> Result<(), ApiError> {
    let ongoing_indexation = state.ongoing_indexation.write().await;
    let idx_process = ongoing_indexation.get(&repo_data.full_name);
    if idx_process.is_some() {
        return Err(ApiError::IndexationInProgress);
    }
    state.tx.send(EventData::Indexation(repo_data)).await?;
    Ok(())
}

pub async fn regenerate_embeddings(
    SecretValidator: SecretValidator,
    State(state): State<AppState>,
) -> Result<(), ApiError> {
    state.tx.send(EventData::RegenerateEmbeddings).await?;
    Ok(())
}

pub async fn health() -> impl IntoResponse {
    if !PRE_SHUTDOWN.load(Ordering::SeqCst) {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}

#[cfg(test)]
mod tests {
    use std::{borrow::BorrowMut, collections::HashMap, sync::Arc};

    use axum::{
        body::Body,
        http::{header::CONTENT_TYPE, Request, StatusCode},
    };
    use tokio::sync::{mpsc, RwLock};
    use tower::ServiceExt;

    use crate::{
        app,
        config::{load_config, IssueBotConfig},
        AppState,
    };

    #[tokio::test]
    async fn test_github_webhook_handler() {
        let config: IssueBotConfig = load_config("ISSUE_BOT_TEST").unwrap();
        let (tx, _rx) = mpsc::channel(8);
        let state = AppState {
            auth_token: config.auth_token.clone(),
            ongoing_indexation: Arc::new(RwLock::new(HashMap::new())),
            tx,
        };
        let mut app = app(state);

        let payload_body = r#"{"action":"opened","issue":{"title":"my great contribution to the world","body":"superb work, isnt it","id":4321,"number":5,"html_url":"https://github.com/huggingface/lor-e/5", "url":"https://github.com/api/huggingface/lor-e/5"}, "repository":{"full_name":"huggingface/lor-e"}}"#;
        let sig = "sha256=8e288dccf7b2744c5f3f30ab1e82672f16c0cb0f809d384df85cac2421e153af";

        let response = app
            .borrow_mut()
            .oneshot(
                Request::builder()
                    .method(axum::http::Method::POST)
                    .uri("/event/github")
                    .header("x-hub-signature-256", sig)
                    .body(Body::from(payload_body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let payload_body = r#"{"action":"created","comment":{"body":"test review","id":1234,"url":"https://github.com/huggingface/lor-e/5#comment-123"},"issue":{"title":"my great contribution to the world","body":"superb work, isnt it","id":4321,"number":5,"html_url":"https://github.com/huggingface/lor-e/5", "url":"https://github.com/api/huggingface/lor-e/5"}, "repository":{"full_name":"huggingface/lor-e"}}"#;
        let sig = "sha256=017815fdb6eda66aa8f62123844001fa64e1b2c137808a0ac68f60091ca36f56";

        let response = app
            .oneshot(
                Request::builder()
                    .method(axum::http::Method::POST)
                    .uri("/event/github")
                    .header("x-hub-signature-256", sig)
                    .body(Body::from(payload_body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_hf_webhook_handler() {
        let config: IssueBotConfig = load_config("ISSUE_BOT_TEST").unwrap();
        let auth_token = config.auth_token.clone();
        let (tx, _rx) = mpsc::channel(8);
        let state = AppState {
            auth_token: auth_token.clone(),
            ongoing_indexation: Arc::new(RwLock::new(HashMap::new())),
            tx,
        };
        let mut app = app(state);

        let payload_body = r#"{"event":{"action":"create", "scope":"discussion"}, "discussion":{"id":"test", "isPullRequest":false, "num":1, "title":"my test issue","url":{"api":"https://huggingface.co/test", "web":"https://huggingface.co/test"}}}"#;

        let response = app
            .borrow_mut()
            .oneshot(
                Request::builder()
                    .method(axum::http::Method::POST)
                    .uri("/event/huggingface")
                    .header("x-webhook-secret", &auth_token)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(payload_body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let payload_body = r#"{"event":{"action":"create", "scope":"discussion.comment"}, "discussion":{"id":"test", "isPullRequest":false, "num":1, "title":"my test issue","url":{"api":"https://huggingface.co/test", "web":"https://huggingface.co/test"}}, "comment":{"id":"test", "content":"some comment", "author":{"id":"test"},"url":{"web":"https://huggingface.co/test"}}}"#;

        let response = app
            .oneshot(
                Request::builder()
                    .method(axum::http::Method::POST)
                    .uri("/event/huggingface")
                    .header("x-webhook-secret", &auth_token)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(payload_body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }
}
