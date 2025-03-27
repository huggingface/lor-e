use std::{env, fmt::Display, sync::Once, time::Duration};

use axum::{
    error_handling::HandleErrorLayer,
    http::{Response, StatusCode},
    middleware,
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use chrono::Utc;
use config::{load_config, IssueBotConfig, ServerConfig};
use embeddings::inference_endpoints::EmbeddingApi;
use github::GithubApi;
use huggingface::HuggingfaceApi;
use metrics::start_metrics_server;
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use middlewares::RequestSpan;
use pgvector::Vector;
use routes::index_repository;
use serde::Deserialize;
use sqlx::{
    postgres::{PgConnectOptions, PgPoolOptions},
    prelude::FromRow,
    Pool, Postgres,
};
use tokio::{
    net::TcpListener,
    sync::mpsc::{self, Receiver, Sender},
    task::JoinHandle,
};
use tower::{BoxError, ServiceBuilder};
use tower_http::trace::TraceLayer;
use tracing::{info, Span};
use tracing_subscriber::EnvFilter;

mod config;
mod embeddings;
mod errors;
mod github;
mod huggingface;
mod metrics;
mod middlewares;
mod routes;

#[derive(Clone)]
pub struct AppState {
    auth_token: String,
    tx: Sender<EventData>,
}

fn setup_metrics_recorder() -> PrometheusHandle {
    const EXPONENTIAL_SECONDS: &[f64] = &[
        0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
    ];

    PrometheusBuilder::new()
        .set_buckets_for_metric(
            Matcher::Full("issue_bot_api_response_time_hist".to_string()),
            EXPONENTIAL_SECONDS,
        )
        .unwrap()
        .install_recorder()
        .unwrap()
}

/// [init_logging] must only be called once, tests may try to call it multiple times
static ONCE_LOGGING: Once = Once::new();

/// Init logging using env variables LOG_LEVEL and LOG_FORMAT
/// LOG_LEVEL may be TRACE, DEBUG, INFO, WARN or ERROR (default to INFO)
/// LOG_FORMAT may be TEXT or JSON (default to TEXT)
pub fn init_logging() {
    ONCE_LOGGING.call_once(|| {
        let builder = tracing_subscriber::fmt()
            .with_target(true)
            .with_line_number(true)
            .with_env_filter(
                EnvFilter::try_from_env("LOG_LEVEL").unwrap_or_else(|_| EnvFilter::new("info")),
            );
        let json = env::var("LOG_FORMAT")
            .map(|value| value.to_lowercase() == "json")
            .unwrap_or(false);
        if json {
            builder
                .json()
                .flatten_event(true)
                .with_current_span(false)
                .with_span_list(true)
                .init()
        } else {
            builder.init()
        }
    });
}

pub async fn flatten(handle: JoinHandle<anyhow::Result<()>>) -> anyhow::Result<()> {
    match handle.await {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(err)) => Err(err),
        Err(err) => Err(anyhow::anyhow!("handling failed: {err}")),
    }
}

fn app(state: AppState) -> Router {
    Router::new()
        .nest("/event", routes::event_router())
        .route("/index", post(index_repository))
        .route_layer(middleware::from_fn(middlewares::track_metrics))
        .layer(
            ServiceBuilder::new()
                .layer(HandleErrorLayer::new(|error: BoxError| async move {
                    if error.is::<tower::timeout::error::Elapsed>() {
                        Ok(StatusCode::REQUEST_TIMEOUT)
                    } else {
                        Err((
                            StatusCode::INTERNAL_SERVER_ERROR,
                            format!("Unhandled internal error: {error}"),
                        ))
                    }
                }))
                .timeout(Duration::from_secs(10))
                .layer(
                    TraceLayer::new_for_http()
                        .make_span_with(RequestSpan)
                        .on_response(|res: &Response<_>, latency: Duration, _span: &Span| {
                            info!(
                                latency_micros = latency.as_micros(),
                                status_code = res.status().as_u16(),
                            )
                        }),
                )
                .into_inner(),
        )
        .layer(middleware::from_fn(middlewares::add_request_id))
        .route("/health", get(|| async { StatusCode::OK.into_response() }))
        .with_state(state)
}

async fn start_main_server(config: ServerConfig, state: AppState) -> anyhow::Result<()> {
    let addr = format!("{}:{}", config.ip, config.port);
    info!(addr, "starting server");

    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, app(state)).await?;

    Ok(())
}

struct IssueData {
    source_id: String,
    action: Action,
    title: String,
    body: String,
    is_pull_request: bool,
    number: i32,
    html_url: String,
    url: String,
    source: Source,
}

struct CommentData {
    source_id: String,
    action: Action,
    issue_id: String,
    body: String,
    url: String,
}

#[derive(Clone, Deserialize)]
pub struct RepositoryData {
    repo_id: String,
    source: Source,
}

impl Display for RepositoryData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} repo '{}'", self.source, self.repo_id)
    }
}

enum EventData {
    Issue(IssueData),
    Comment(CommentData),
    Indexation(RepositoryData),
}

enum Action {
    Created,
    Edited,
    Deleted,
}

impl Display for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let action = match self {
            Self::Created => "created",
            Self::Edited => "edited",
            Self::Deleted => "deleted",
        };
        write!(f, "{}", action)
    }
}

#[derive(Clone, Deserialize)]
enum Source {
    Github,
    HuggingFace,
}

impl Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let source = match self {
            Self::Github => "Github",
            Self::HuggingFace => "HuggingFace",
        };
        write!(f, "{}", source)
    }
}

#[derive(FromRow)]
struct ClosestIssue {
    title: String,
    number: i32,
    html_url: String,
    cosine_similarity: i32,
}

async fn handle_webhooks(
    mut rx: Receiver<EventData>,
    embedding_api: EmbeddingApi,
    github_api: GithubApi,
    huggingface_api: HuggingfaceApi,
    pool: Pool<Postgres>,
) -> anyhow::Result<()> {
    while let Some(webhook_data) = rx.recv().await {
        let now = Utc::now();
        let issue_id = match webhook_data {
            EventData::Issue(issue) => {
                info!("handling issue (state: {})", issue.action);
                match issue.action {
                    Action::Created => {
                        let issue_text = format!("# {}\n{}", issue.title, issue.body);
                        let embedding =
                            Vector::from(embedding_api.generate_embedding(issue_text).await?);

                        let closest_issues: Vec<ClosestIssue> = sqlx::query_as(
                            "select title, number, html_url, 1 - (embedding <=> $1) as cosine_similarity from issues order by embedding <=> $1 LIMIT 3",
                        )
                            .bind(embedding.clone())
                            .fetch_all(&pool)
                            .await?;

                        match (issue.is_pull_request, &issue.source) {
                            (false, Source::Github) => {
                                github_api
                                    .comment_on_issue(&issue.url, closest_issues)
                                    .await?;
                            }
                            (false, Source::HuggingFace) => {
                                huggingface_api
                                    .comment_on_issue(&issue.url, closest_issues)
                                    .await?;
                            }
                            _ => (),
                        }

                        sqlx::query(
                        r#"insert into issues (source_id, source, title, body, is_pull_request, number, html_url, url, embedding, created_at, updated_at)
                           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)"#
                        )
                        .bind(issue.source_id)
                        .bind(issue.source.to_string())
                        .bind(issue.title)
                        .bind(issue.body)
                        .bind(issue.is_pull_request)
                        .bind(issue.number)
                        .bind(issue.html_url)
                        .bind(issue.url)
                        .bind(embedding)
                        .bind(now)
                        .bind(now)
                        .execute(&pool)
                        .await?;

                        None
                    }
                    Action::Edited => {
                        sqlx::query!(
                            r#"update issues
                           set title = $1, body = $2, url = $3, updated_at = $4
                           where source_id = $5"#,
                            issue.title,
                            issue.body,
                            issue.url,
                            now,
                            issue.source_id,
                        )
                        .execute(&pool)
                        .await?;
                        Some(issue.source_id)
                    }
                    Action::Deleted => {
                        sqlx::query!(
                            r#"DELETE FROM issues WHERE source_id = $1"#,
                            issue.source_id
                        )
                        .execute(&pool)
                        .await?;
                        None
                    }
                }
            }
            EventData::Comment(comment) => {
                info!("handling comment (state: {})", comment.action);
                match comment.action {
                    Action::Created => {
                        sqlx::query!(
                            r#"insert into comments (source_id, body, url, created_at, updated_at, issue_id)
                               values ($1, $2, $3, $4, $5,
                                      (select id from issues where source_id = $6))"#,
                            comment.source_id,
                            comment.body,
                            comment.url,
                            now,
                            now,
                            comment.issue_id,
                        )
                        .execute(&pool)
                        .await?;
                        Some(comment.issue_id)
                    }
                    Action::Edited => {
                        sqlx::query!(
                            r#"update comments
                           set body = $1, url = $2, updated_at = $3
                           where source_id = $4"#,
                            comment.body,
                            comment.url,
                            now,
                            comment.source_id,
                        )
                        .execute(&pool)
                        .await?;
                        Some(comment.issue_id)
                    }
                    Action::Deleted => {
                        sqlx::query!(
                            r#"DELETE FROM comments WHERE source_id = $1"#,
                            comment.source_id
                        )
                        .execute(&pool)
                        .await?;
                        Some(comment.issue_id)
                    }
                }
            }
            EventData::Indexation(repository) => {
                info!("indexing {repository}");
                let (issues_tx, mut issues_rx) = mpsc::unbounded_channel();
                let github_api = github_api.clone();
                let repository_clone = repository.clone();
                let handle = tokio::spawn(async move {
                    github_api.get_issues(issues_tx, repository_clone).await
                });
                // TODO: parallelize
                while let Some(issue) = issues_rx.recv().await {
                    let comment_string = format!(
                        "\n----\nComment: {}",
                        issue
                            .comments
                            .into_iter()
                            .map(|c| c.body.to_owned())
                            .collect::<Vec<String>>()
                            .join("\n----\nComment: ")
                    );
                    let issue_text = format!("# {}\n{}{}", issue.title, issue.body, comment_string);
                    let embedding =
                        Vector::from(embedding_api.generate_embedding(issue_text).await?);
                    let now = Utc::now();
                    sqlx::query(
                        r#"insert into issues (source_id, source, title, body, is_pull_request, number, html_url, url, embedding, created_at, updated_at)
                           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)"#
                        )
                        .bind(issue.id.to_string())
                        .bind(repository.source.to_string())
                        .bind(issue.title)
                        .bind(issue.body)
                        .bind(issue.is_pull_request)
                        .bind(issue.number)
                        .bind(issue.html_url)
                        .bind(issue.url)
                        .bind(embedding)
                        .bind(now)
                        .bind(now)
                        .execute(&pool)
                        .await?;
                    info!("indexed issue #{}", issue.number);
                }
                handle.await??;
                None
            }
        };

        if let Some(issue_id) = issue_id {
            let issue = sqlx::query!(
                r#"
                SELECT
                  i.title,
                  i.body,
                  (
                    SELECT JSON_AGG(c.body)
                    FROM comments AS c
                    WHERE c.issue_id = i.id
                  ) AS comments
                FROM
                  issues AS i
                WHERE
                  i.source_id = $1;
            "#,
                issue_id,
            )
            .fetch_one(&pool)
            .await?;
            let comment_string = match issue.comments {
                Some(comments) => {
                    let comments: Vec<String> = serde_json::from_value(comments)?;
                    format!("\n----\nComment: {}", comments.join("\n----\nComment: "))
                }
                None => String::new(),
            };
            let issue_text = format!("# {}\n{}{}", issue.title, issue.body, comment_string);
            let embedding = Vector::from(embedding_api.generate_embedding(issue_text).await?);
            sqlx::query(
                r#"update issues
               set embedding = $1
               where source_id = $2"#,
            )
            .bind(embedding)
            .bind(issue_id)
            .execute(&pool)
            .await?;
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_logging();

    let config: IssueBotConfig = load_config("ISSUE_BOT")?;

    let opts: PgConnectOptions = config.database.connection_string.parse()?;
    let pool = PgPoolOptions::new()
        .max_connections(config.database.max_connections)
        .connect_with(opts)
        .await?;

    let embedding_api = EmbeddingApi::new(config.model_api).await?;
    let github_api = GithubApi::new(config.github_api, config.message_config.clone())?;
    let huggingface_api = HuggingfaceApi::new(config.huggingface_api, config.message_config)?;

    let (tx, rx) = mpsc::channel(4_096);

    let state = AppState {
        auth_token: config.auth_token,
        tx,
    };

    let host = config.server.ip.clone();
    let metrics_port = config.server.metrics_port;

    tokio::try_join!(
        start_main_server(config.server, state),
        flatten(tokio::spawn(start_metrics_server(
            host,
            metrics_port,
            false,
            setup_metrics_recorder()
        ))),
        handle_webhooks(rx, embedding_api, github_api, huggingface_api, pool)
    )?;

    Ok(())
}
