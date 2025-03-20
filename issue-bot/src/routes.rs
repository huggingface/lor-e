use std::fmt::Display;

use axum::{
    body::Body,
    extract::{FromRef, FromRequestParts, Request, State},
    http::{request::Parts, HeaderName},
    routing::post,
    Json, Router,
};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tracing::info;

use crate::{errors::ApiError, Action, AppState, Source, WebhookData};

fn compute_signature(payload: &[u8], secret: &str) -> String {
    let key = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    let mut mac = key;
    mac.update(payload);
    let result = mac.finalize().into_bytes();
    format!("sha256={}", hex::encode(result))
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommentActionType {
    Created,
    Deleted,
    Edited,
}

impl Display for CommentActionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.serialize(f)
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewActionType {
    Dismissed,
    Edited,
    Submitted,
}

impl Display for ReviewActionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.serialize(f)
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueActionType {
    Opened,
    Edited,
    Deleted,
    /// We don't care about other action types
    #[serde(other)]
    Ignored,
}

impl Display for IssueActionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.serialize(f)
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PullRequestActionType {
    Opened,
    Edited,
    /// We don't care about other action types
    #[serde(other)]
    Ignored,
}

impl Display for PullRequestActionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.serialize(f)
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Comment {
    body: String,
    id: i64,
    url: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Issue {
    action: IssueActionType,
    issue: IssueData,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct IssueData {
    body: String,
    id: i64,
    number: i32,
    title: String,
    url: String,
}

/// Issue & Pull Request comments
#[derive(Debug, Deserialize, Serialize)]
pub struct IssueComment {
    action: CommentActionType,
    comment: Comment,
    issue: IssueData,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PullRequest {
    action: PullRequestActionType,
    pull_request: PullRequestData,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PullRequestData {
    body: String,
    id: i64,
    number: i32,
    title: String,
    url: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Review {
    body: String,
    id: i64,
    url: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PullRequestReview {
    action: ReviewActionType,
    pull_request: PullRequestData,
    review: Review,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PullRequestReviewComment {
    action: CommentActionType,
    comment: Comment,
    pull_request: PullRequestData,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum GithubWebhook {
    IssueComment(IssueComment),
    Issue(Issue),
    PullRequestReviewComment(PullRequestReviewComment),
    PullRequestReview(PullRequestReview),
    PullRequest(PullRequest),
}

impl Display for GithubWebhook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let webhook_type = match self {
            Self::Issue(_) => "issue",
            Self::IssueComment(_) => "issue comment",
            Self::PullRequest(_) => "pull request",
            Self::PullRequestReview(_) => "pull request review",
            Self::PullRequestReviewComment(_) => "pull request review comment",
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
    let webhook_type = webhook.to_string();
    match webhook {
        GithubWebhook::Issue(issue) => {
            info!("received {} (state: {})", webhook_type, issue.action);
            match issue.action {
                IssueActionType::Opened => {
                    state
                        .tx
                        .send(WebhookData::Issue(crate::IssueData {
                            source_id: issue.issue.id.to_string(),
                            action: Action::Created,
                            title: issue.issue.title,
                            body: issue.issue.body,
                            is_pull_request: false,
                            number: issue.issue.number,
                            url: issue.issue.url,
                            source: Source::Github,
                        }))
                        .await?
                }
                IssueActionType::Edited => {
                    state
                        .tx
                        .send(WebhookData::Issue(crate::IssueData {
                            source_id: issue.issue.id.to_string(),
                            action: Action::Edited,
                            title: issue.issue.title,
                            body: issue.issue.body,
                            is_pull_request: false,
                            number: issue.issue.number,
                            url: issue.issue.url,
                            source: Source::Github,
                        }))
                        .await?
                }

                // FIXME: delete associated comments, reviews & review comments
                IssueActionType::Deleted => {
                    state
                        .tx
                        .send(WebhookData::Issue(crate::IssueData {
                            source_id: issue.issue.id.to_string(),
                            action: Action::Deleted,
                            title: issue.issue.title,
                            body: issue.issue.body,
                            is_pull_request: false,
                            number: issue.issue.number,
                            url: issue.issue.url,
                            source: Source::Github,
                        }))
                        .await?
                }
                IssueActionType::Ignored => (),
            }
        }
        GithubWebhook::IssueComment(comment) => {
            info!("received {} (state: {})", webhook_type, comment.action);
            match comment.action {
                CommentActionType::Created => {
                    state
                        .tx
                        .send(WebhookData::Comment(crate::CommentData {
                            source_id: comment.comment.id.to_string(),
                            issue_id: comment.issue.id.to_string(),
                            action: Action::Created,
                            body: comment.comment.body,
                            url: comment.comment.url,
                        }))
                        .await?
                }
                CommentActionType::Edited => {
                    state
                        .tx
                        .send(WebhookData::Comment(crate::CommentData {
                            source_id: comment.comment.id.to_string(),
                            issue_id: comment.issue.id.to_string(),
                            action: Action::Edited,
                            body: comment.comment.body,
                            url: comment.comment.url,
                        }))
                        .await?
                }
                CommentActionType::Deleted => {
                    state
                        .tx
                        .send(WebhookData::Comment(crate::CommentData {
                            source_id: comment.comment.id.to_string(),
                            issue_id: comment.issue.id.to_string(),
                            action: Action::Deleted,
                            body: comment.comment.body,
                            url: comment.comment.url,
                        }))
                        .await?
                }
            }
        }
        GithubWebhook::PullRequest(pr) => {
            info!("received {} (state: {})", webhook_type, pr.action);
            match pr.action {
                PullRequestActionType::Opened => {
                    state
                        .tx
                        .send(WebhookData::Issue(crate::IssueData {
                            source_id: pr.pull_request.id.to_string(),
                            action: Action::Created,
                            title: pr.pull_request.title,
                            body: pr.pull_request.body,
                            is_pull_request: true,
                            number: pr.pull_request.number,
                            url: pr.pull_request.url,
                            source: Source::Github,
                        }))
                        .await?
                }
                PullRequestActionType::Edited => {
                    state
                        .tx
                        .send(WebhookData::Issue(crate::IssueData {
                            source_id: pr.pull_request.id.to_string(),
                            action: Action::Edited,
                            title: pr.pull_request.title,
                            body: pr.pull_request.body,
                            is_pull_request: true,
                            number: pr.pull_request.number,
                            url: pr.pull_request.url,
                            source: Source::Github,
                        }))
                        .await?
                }
                PullRequestActionType::Ignored => (),
            }
        }
        GithubWebhook::PullRequestReview(review) => {
            info!("received {} (state: {})", webhook_type, review.action);
            match review.action {
                ReviewActionType::Submitted => {
                    state
                        .tx
                        .send(WebhookData::Comment(crate::CommentData {
                            source_id: review.review.id.to_string(),
                            issue_id: review.pull_request.id.to_string(),
                            action: Action::Created,
                            body: review.review.body,
                            url: review.review.url,
                        }))
                        .await?
                }
                ReviewActionType::Edited => {
                    state
                        .tx
                        .send(WebhookData::Comment(crate::CommentData {
                            source_id: review.review.id.to_string(),
                            issue_id: review.pull_request.id.to_string(),
                            action: Action::Edited,
                            body: review.review.body,
                            url: review.review.url,
                        }))
                        .await?
                }
                ReviewActionType::Dismissed => (),
            }
        }
        GithubWebhook::PullRequestReviewComment(comment) => {
            info!("received {} (state: {})", webhook_type, comment.action);
            match comment.action {
                CommentActionType::Created => {
                    state
                        .tx
                        .send(WebhookData::Comment(crate::CommentData {
                            source_id: comment.comment.id.to_string(),
                            issue_id: comment.pull_request.id.to_string(),
                            action: Action::Created,
                            body: comment.comment.body,
                            url: comment.comment.url,
                        }))
                        .await?
                }
                CommentActionType::Edited => {
                    state
                        .tx
                        .send(WebhookData::Comment(crate::CommentData {
                            source_id: comment.comment.id.to_string(),
                            issue_id: comment.pull_request.id.to_string(),
                            action: Action::Edited,
                            body: comment.comment.body,
                            url: comment.comment.url,
                        }))
                        .await?
                }
                CommentActionType::Deleted => {
                    state
                        .tx
                        .send(WebhookData::Comment(crate::CommentData {
                            source_id: comment.comment.id.to_string(),
                            issue_id: comment.pull_request.id.to_string(),
                            action: Action::Deleted,
                            body: comment.comment.body,
                            url: comment.comment.url,
                        }))
                        .await?
                }
            }
        }
    }

    Ok(())
}

pub struct HfWebhookSecretValidator;

impl<S> FromRequestParts<S> for HfWebhookSecretValidator
where
    AppState: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let header_name = HeaderName::from_static("x-webhook-secret");
        let state = AppState::from_ref(state);
        let secret = parts
            .headers
            .get(header_name)
            .cloned()
            .ok_or(ApiError::Auth)?;

        if secret != state.auth_token {
            return Err(ApiError::Auth);
        }

        Ok(HfWebhookSecretValidator)
    }
}

#[derive(Debug, Deserialize)]
pub struct HuggingfaceWebhook {
    tmp: String,
}

pub async fn huggingface_webhook(
    HfWebhookSecretValidator: HfWebhookSecretValidator,
    State(state): State<AppState>,
    Json(webhook): Json<HuggingfaceWebhook>,
) -> Result<(), ApiError> {
    info!("{webhook:?}");

    Ok(())
}

pub fn event_router() -> Router<AppState> {
    Router::new()
        .route("/github", post(github_webhook))
        .route("/huggingface", post(huggingface_webhook))
}

#[cfg(test)]
mod tests {
    use std::borrow::BorrowMut;

    use axum::{
        body::Body,
        http::{header::CONTENT_TYPE, Request, StatusCode},
    };
    use tokio::sync::mpsc;
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
            tx,
        };
        let mut app = app(state);
        let payload_body = r#"{"action":"opened","pull_request":{"title":"my great contribution to the world","body":"superb work, isnt it","id":4321,"url":"https://github.com/huggingface/lor-e/5"}}"#;
        let sig = "sha256=a2754571daeaa409f7daaf295dc553ac9251ee97c9fe4de0f6deb02653a26136";

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

        let payload_body = r#"{"action":"submitted","review":{"body":"test review","id":1234,"url":"https://github.com/huggingface/lor-e/5#comment-123"},"pull_request":{"title":"my great contribution to the world","body":"superb work, isnt it","id":4321,"url":"https://github.com/huggingface/lor-e/5"}}"#;
        let sig = "sha256=3e9f1ebb1c08ed75b6d6a174a28bd9771ee062623ad9f3118199e00922bf098d";

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
    }

    #[tokio::test]
    async fn test_hf_webhook_handler() {
        let config: IssueBotConfig = load_config("ISSUE_BOT_TEST").unwrap();
        let auth_token = config.auth_token.clone();
        let (tx, _) = mpsc::channel(8);
        let state = AppState {
            auth_token: auth_token.clone(),
            tx,
        };
        let app = app(state);
        let payload_body = r#"{"tmp":"bob"}"#;

        let response = app
            .oneshot(
                Request::builder()
                    .method(axum::http::Method::POST)
                    .uri("/event/huggingface")
                    .header("x-webhook-secret", auth_token)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(payload_body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }
}
