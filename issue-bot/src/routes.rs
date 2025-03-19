use std::fmt::Display;

use axum::{
    body::Body,
    extract::{FromRef, FromRequestParts, Request, State},
    http::{request::Parts, HeaderName},
    routing::post,
    Json, Router,
};
use chrono::Utc;
use hmac::{Hmac, Mac};
use pgvector::Vector;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tracing::info;

use crate::{errors::ApiError, AppState};

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

#[derive(Deserialize)]
struct CommentBody {
    body: String,
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
    let now = Utc::now();
    let webhook_type = webhook.to_string();
    let mut issue_id: Option<i64> = None;
    match webhook {
        GithubWebhook::Issue(issue) => {
            info!("handling {} (state: {})", webhook_type, issue.action);
            match issue.action {
                IssueActionType::Opened => {
                    let issue_text = format!("# {}\n{}", issue.issue.title, issue.issue.body);
                    let embedding =
                        Vector::from(state.embedding_api.generate_embedding(issue_text).await?);
                    sqlx::query(
                        r#"insert into issues (github_id, title, body, issue_type, url, embedding, created_at, updated_at)
                           values ($1, $2, $3, $4, $5, $6, $7, $8)"#
                    )
                    .bind(issue.issue.id)
                    .bind(issue.issue.title)
                    .bind(issue.issue.body)
                    .bind("issue")
                    .bind(issue.issue.url)
                    .bind(embedding)
                    .bind(now)
                    .bind(now)
                    .execute(&state.pool)
                    .await?;
                }
                IssueActionType::Edited => {
                    issue_id = Some(issue.issue.id);
                    sqlx::query!(
                        r#"update issues
                           set title = $1, body = $2, url = $3, updated_at = $4
                           where github_id = $5"#,
                        issue.issue.title,
                        issue.issue.body,
                        issue.issue.url,
                        now,
                        issue.issue.id,
                    )
                    .execute(&state.pool)
                    .await?;
                }
                // FIXME: delete associated comments, reviews & review comments
                IssueActionType::Deleted => {
                    sqlx::query!(r#"DELETE FROM issues WHERE github_id = $1"#, issue.issue.id)
                        .execute(&state.pool)
                        .await?;
                }
                IssueActionType::Ignored => (),
            }
        }
        GithubWebhook::IssueComment(comment) => {
            info!("handling {} (state: {})", webhook_type, comment.action);
            match comment.action {
                CommentActionType::Created => {
                    issue_id = Some(comment.issue.id);
                    sqlx::query!(
                        r#"insert into issue_comments (github_id, body, url, created_at, updated_at, issue_id)
                           values ($1, $2, $3, $4, $5,
                                  (select id from issues where github_id = $6))"#,
                        comment.comment.id,
                        comment.comment.body,
                        comment.comment.url,
                        now,
                        now,
                        comment.issue.id,
                    )
                    .execute(&state.pool)
                    .await?;
                }
                CommentActionType::Edited => {
                    issue_id = Some(comment.issue.id);
                    sqlx::query!(
                        r#"update issue_comments
                           set body = $1, url = $2, updated_at = $3
                           where github_id = $4"#,
                        comment.comment.body,
                        comment.comment.url,
                        now,
                        comment.comment.id,
                    )
                    .execute(&state.pool)
                    .await?;
                }
                CommentActionType::Deleted => {
                    issue_id = Some(comment.issue.id);
                    sqlx::query!(
                        r#"DELETE FROM issue_comments WHERE github_id = $1"#,
                        comment.comment.id
                    )
                    .execute(&state.pool)
                    .await?;
                }
            }
        }
        GithubWebhook::PullRequest(pr) => {
            info!("handling {} (state: {})", webhook_type, pr.action);
            match pr.action {
                PullRequestActionType::Opened => {
                    issue_id = Some(pr.pull_request.id);
                    sqlx::query!(
                        r#"insert into issues (github_id, title, body, issue_type, url, created_at, updated_at)
                           values ($1, $2, $3, $4, $5, $6, $7)"#,
                        pr.pull_request.id,
                        pr.pull_request.title,
                        pr.pull_request.body,
                        "pull_request",
                        pr.pull_request.url,
                        now,
                        now
                    )
                    .execute(&state.pool)
                    .await?;
                }
                PullRequestActionType::Edited => {
                    issue_id = Some(pr.pull_request.id);
                    sqlx::query!(
                        r#"update issues
                           set title = $1, body = $2, url = $3, updated_at = $4
                           where github_id = $5"#,
                        pr.pull_request.title,
                        pr.pull_request.body,
                        pr.pull_request.url,
                        now,
                        pr.pull_request.id,
                    )
                    .execute(&state.pool)
                    .await?;
                }
                PullRequestActionType::Ignored => (),
            }
        }
        GithubWebhook::PullRequestReview(review) => {
            info!("handling {} (state: {})", webhook_type, review.action);
            match review.action {
                ReviewActionType::Submitted => {
                    issue_id = Some(review.pull_request.id);
                    sqlx::query!(
                        r#"insert into pull_request_reviews (github_id, body, url, created_at, updated_at, issue_id)
                           values ($1, $2, $3, $4, $5,
                                  (select id from issues where github_id = $6))"#,
                        review.review.id,
                        review.review.body,
                        review.review.url,
                        now,
                        now,
                        review.pull_request.id,
                    )
                    .execute(&state.pool)
                    .await?;
                }
                ReviewActionType::Edited => {
                    issue_id = Some(review.pull_request.id);
                    sqlx::query!(
                        r#"update pull_request_reviews
                           set body = $1, url = $2, updated_at = $3
                           where github_id = $4"#,
                        review.review.body,
                        review.review.url,
                        now,
                        review.review.id,
                    )
                    .execute(&state.pool)
                    .await?;
                }
                ReviewActionType::Dismissed => (),
            }
        }
        GithubWebhook::PullRequestReviewComment(comment) => {
            info!("handling {} (state: {})", webhook_type, comment.action);
            match comment.action {
                CommentActionType::Created => {
                    issue_id = Some(comment.pull_request.id);
                    sqlx::query!(
                        r#"insert into pull_request_review_comments (github_id, body, url, created_at, updated_at, issue_id)
                           values ($1, $2, $3, $4, $5,
                                  (select id from issues where github_id = $6))"#,
                        comment.comment.id,
                        comment.comment.body,
                        comment.comment.url,
                        now,
                        now,
                        comment.pull_request.id,
                    )
                    .execute(&state.pool)
                    .await?;
                }
                CommentActionType::Edited => {
                    issue_id = Some(comment.pull_request.id);
                    sqlx::query!(
                        r#"update pull_request_review_comments
                           set body = $1, url = $2, updated_at = $3
                           where github_id = $4"#,
                        comment.comment.body,
                        comment.comment.url,
                        now,
                        comment.comment.id,
                    )
                    .execute(&state.pool)
                    .await?;
                }
                CommentActionType::Deleted => {
                    issue_id = Some(comment.pull_request.id);
                    sqlx::query!(
                        r#"DELETE FROM pull_request_review_comments WHERE github_id = $1"#,
                        comment.comment.id
                    )
                    .execute(&state.pool)
                    .await?;
                }
            }
        }
    }

    if let Some(issue_id) = issue_id {
        let issue = sqlx::query!(
            r#"
                SELECT
                  i.title,
                  i.body,
                  (
                    SELECT JSON_AGG(json_build_object('body', ic.body))
                    FROM issue_comments AS ic
                    WHERE ic.issue_id = i.id
                  ) AS issue_comments,
                  (
                    SELECT JSON_AGG(json_build_object('body', prr.body))
                    FROM pull_request_reviews AS prr
                    WHERE prr.issue_id = i.id
                  ) AS pull_request_reviews,
                  (
                    SELECT JSON_AGG(json_build_object('body', prrc.body))
                    FROM pull_request_review_comments AS prrc
                    WHERE prrc.issue_id = i.id
                  ) AS pull_request_review_comments
                FROM
                  issues AS i
                WHERE
                  i.github_id = $1;
            "#,
            issue_id,
        )
        .fetch_one(&state.pool)
        .await?;
        let comment_string = [
            issue.issue_comments,
            issue.pull_request_reviews,
            issue.pull_request_review_comments,
        ]
        .into_iter()
        .flat_map(|opt| opt.map(serde_json::from_value::<Vec<CommentBody>>))
        .flatten()
        .flatten()
        .map(|b| b.body)
        .collect::<Vec<String>>()
        .join("\n----\nComment: ");
        let issue_text = format!(
            "# {}\n{}\n----\nComment: {}",
            issue.title, issue.body, comment_string
        );
        let embedding = Vector::from(state.embedding_api.generate_embedding(issue_text).await?);
        sqlx::query(
            r#"update issues
               set embedding = $1
               where github_id = $2"#,
        )
        .bind(embedding)
        .bind(issue_id)
        .execute(&state.pool)
        .await?;
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
    use sqlx::{Pool, Postgres};
    use tower::ServiceExt;

    use crate::{
        app,
        config::{load_config, IssueBotConfig},
        embeddings::inference_endpoints::EmbeddingApi,
        AppState,
    };

    #[sqlx::test(fixtures("lor_e"))]
    async fn test_github_webhook_handler(pool: Pool<Postgres>) {
        let config: IssueBotConfig = load_config("ISSUE_BOT_TEST").unwrap();
        let embedding_api = EmbeddingApi::new(config.model_api).await.unwrap();
        let state = AppState {
            auth_token: config.auth_token.clone(),
            embedding_api,
            pool,
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

    #[sqlx::test]
    async fn test_hf_webhook_handler(pool: Pool<Postgres>) {
        let config: IssueBotConfig = load_config("ISSUE_BOT_TEST").unwrap();
        let embedding_api = EmbeddingApi::new(config.model_api).await.unwrap();
        let auth_token = config.auth_token.clone();
        let state = AppState {
            auth_token: auth_token.clone(),
            embedding_api,
            pool,
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
