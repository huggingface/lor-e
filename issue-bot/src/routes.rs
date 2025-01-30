use axum::{
    body::Body,
    extract::{FromRef, FromRequestParts, Request, State},
    http::{request::Parts, HeaderName},
    routing::post,
    Json, Router,
};
use hmac::{Hmac, Mac};
use serde::Deserialize;
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

#[derive(Debug, Deserialize)]
pub struct GithubWebhook {
    tmp: String,
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
    info!("{webhook:?}");

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
    use axum::{
        body::Body,
        http::{header::CONTENT_TYPE, Request, StatusCode},
    };
    use sqlx::{Pool, Postgres};
    use tower::ServiceExt;

    use crate::{
        app,
        config::{load_config, IssueBotConfig},
        AppState,
    };

    #[sqlx::test]
    async fn test_github_webhook_handler(pool: Pool<Postgres>) {
        let config: IssueBotConfig = load_config("ISSUE_BOT_TEST").unwrap();
        let state = AppState {
            auth_token: config.auth_token.clone(),
            pool,
        };
        let app = app(state);
        let payload_body = r#"{"tmp":"bob"}"#;
        let sig = "sha256=0b415fd16737253068f4b2c6cf30cea7fc9aa640f25aef10063e22b739d41f70";

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

    #[sqlx::test]
    async fn test_hf_webhook_handler(pool: Pool<Postgres>) {
        let config: IssueBotConfig = load_config("ISSUE_BOT_TEST").unwrap();
        let auth_token = config.auth_token.clone();
        let state = AppState {
            auth_token: auth_token.clone(),
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
