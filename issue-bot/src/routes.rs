use std::future::ready;

use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use metrics_exporter_prometheus::PrometheusHandle;
use serde::Deserialize;
use sqlx::{Pool, Postgres};
use tracing::info;

use crate::errors::ApiError;

#[derive(Debug, Deserialize)]
pub struct GithubWebhook {
    tmp: String,
}

pub async fn github_webhook(
    State(pool): State<Pool<Postgres>>,
    Json(webhook): Json<GithubWebhook>,
) -> anyhow::Result<(), ApiError> {
    info!("{webhook:?}");

    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct HuggingfaceWebhook {
    tmp: String,
}

pub async fn huggingface_webhook(
    State(pool): State<Pool<Postgres>>,
    Json(webhook): Json<GithubWebhook>,
) -> Result<(), ApiError> {
    info!("{webhook:?}");

    Ok(())
}

pub fn event_router() -> Router<Pool<Postgres>> {
    Router::new()
        .route("/github", post(github_webhook))
        .route("/huggingface", post(huggingface_webhook))
}

pub fn metrics_router(recorder_handle: PrometheusHandle) -> Router<Pool<Postgres>> {
    Router::new().route("/", get(move || ready(recorder_handle.render())))
}
