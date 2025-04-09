use std::{
    env,
    fmt::Display,
    sync::{
        atomic::{AtomicBool, Ordering},
        Once,
    },
    time::Duration,
};

use axum::{
    error_handling::HandleErrorLayer,
    http::{Response, StatusCode},
    middleware,
    routing::{get, post},
    Router,
};
use config::{load_config, IssueBotConfig, ServerConfig};
use embeddings::inference_endpoints::EmbeddingApi;
use futures::{pin_mut, StreamExt};
use github::GithubApi;
use huggingface::HuggingfaceApi;
use metrics::start_metrics_server;
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use middlewares::RequestSpan;
use pgvector::Vector;
use routes::{health, index_repository};
use serde::{Deserialize, Deserializer};
use slack::Slack;
use sqlx::{
    postgres::{PgConnectOptions, PgPoolOptions},
    prelude::FromRow,
    types::Json,
    Pool, Postgres, QueryBuilder,
};
use summarization::SummarizationApi;
use tokio::{
    net::TcpListener,
    select, signal,
    sync::mpsc::{self, Receiver, Sender},
    task::JoinHandle,
};
use tower::{BoxError, ServiceBuilder};
use tower_http::trace::TraceLayer;
use tracing::{error, info, info_span, Instrument, Span};
use tracing_subscriber::EnvFilter;

mod config;
mod embeddings;
mod errors;
mod github;
mod huggingface;
mod metrics;
mod middlewares;
mod routes;
mod slack;
mod summarization;

static APP_USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"),);

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

pub fn deserialize_null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    T: Default + Deserialize<'de>,
    D: Deserializer<'de>,
{
    let opt = Option::deserialize(deserializer)?;
    Ok(opt.unwrap_or_default())
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
        .route("/health", get(health))
        .with_state(state)
}

async fn start_main_server(config: ServerConfig, state: AppState) -> anyhow::Result<()> {
    let addr = format!("{}:{}", config.ip, config.port);
    info!(addr, "starting server");

    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, app(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;

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

#[derive(Debug, FromRow)]
struct ClosestIssue {
    title: String,
    number: i32,
    html_url: String,
    #[allow(unused)]
    cosine_similarity: f64,
}

#[derive(Debug, Deserialize, FromRow)]
struct JobData {
    issues_page: i32,
}

#[derive(Debug, FromRow)]
struct Job {
    data: Json<JobData>,
}

async fn handle_webhooks_wrapper(
    rx: Receiver<EventData>,
    embedding_api: EmbeddingApi,
    github_api: GithubApi,
    huggingface_api: HuggingfaceApi,
    slack: Slack,
    summarization_api: SummarizationApi,
    pool: Pool<Postgres>,
) -> anyhow::Result<()> {
    select! {
        res = handle_webhooks(rx, embedding_api, github_api, huggingface_api, slack, summarization_api, pool) => { res },
        _ = shutdown_signal() => { Ok(()) },
    }
}

async fn handle_webhooks(
    mut rx: Receiver<EventData>,
    embedding_api: EmbeddingApi,
    github_api: GithubApi,
    huggingface_api: HuggingfaceApi,
    slack: Slack,
    summarization_api: SummarizationApi,
    pool: Pool<Postgres>,
) -> anyhow::Result<()> {
    while let Some(webhook_data) = rx.recv().await {
        let issue_id = match webhook_data {
            EventData::Issue(issue) => {
                info!("handling issue (state: {})", issue.action);
                match issue.action {
                    Action::Created => {
                        let issue_text = format!("# {}\n{}", issue.title, issue.body);
                        let embedding = Vector::from(
                            embedding_api.generate_embedding(issue_text.clone()).await?,
                        );

                        let closest_issues: Vec<ClosestIssue> = sqlx::query_as(
                            "select title, number, html_url, 1 - (embedding <=> $1) as cosine_similarity from issues order by embedding <=> $1 LIMIT 3",
                        )
                            .bind(embedding.clone())
                            .fetch_all(&pool)
                            .await?;

                        let summarized_issue = summarization_api.summarize(issue_text).await?;

                        slack
                            .closest_issues(summarized_issue, &issue, &closest_issues)
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
                        r#"insert into issues (source_id, source, title, body, is_pull_request, number, html_url, url, embedding)
                           values ($1, $2, $3, $4, $5, $6, $7, $8, $9)"#
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
                        .execute(&pool)
                        .await?;

                        None
                    }
                    Action::Edited => {
                        sqlx::query!(
                            r#"update issues
                           set title = $1, body = $2, url = $3, updated_at = current_timestamp
                           where source_id = $4"#,
                            issue.title,
                            issue.body,
                            issue.url,
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
                            r#"insert into comments (source_id, body, url, issue_id)
                               values ($1, $2, $3,
                                      (select id from issues where source_id = $4))"#,
                            comment.source_id,
                            comment.body,
                            comment.url,
                            comment.issue_id,
                        )
                        .execute(&pool)
                        .await?;
                        Some(comment.issue_id)
                    }
                    Action::Edited => {
                        sqlx::query!(
                            r#"update comments
                           set body = $1, url = $2, updated_at = current_timestamp
                           where source_id = $3"#,
                            comment.body,
                            comment.url,
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
                let embedding_api = embedding_api.clone();
                let github_api = github_api.clone();
                let pool = pool.clone();
                let span = info_span!(
                    "indexation",
                    repository_id = repository.repo_id,
                    source = repository.source.to_string()
                );
                tokio::spawn(async move {
                    info!("indexing started");
                    let job = match sqlx::query_as!(
                        Job,
                        r#"select data as "data: Json<JobData>" from jobs where repository_id = $1"#,
                        repository.repo_id
                    )
                    .fetch_optional(&pool)
                    .await {
                        Ok(job) => job,
                        Err(err) => {
                            error!(err = err.to_string(), "error fetching job");
                            return;
                        }
                    };
                    let from_issues_page =
                        job.as_ref().map(|j| j.data.issues_page + 1).unwrap_or(1);
                    let issues = github_api.get_issues(from_issues_page, repository.clone());
                    pin_mut!(issues);
                    while let Some(issue) = issues.next().await {
                        let (issue, page) = match issue {
                            Ok(issue) => issue,
                            Err(err) => {
                                error!(err = err.to_string(), "error fetching next item from issues stream");
                                continue;
                            }
                        };
                        let embedding_api = embedding_api.clone();
                        let pool = pool.clone();
                        let source = repository.source.to_string();
                        let comment_string = format!(
                            "\n----\nComment: {}",
                            issue
                                .comments
                                .iter()
                                .map(|c| c.body.to_owned())
                                .collect::<Vec<String>>()
                                .join("\n----\nComment: ")
                        );
                        let issue_text =
                            format!("# {}\n{}{}", issue.title, issue.body, comment_string);
                        let raw_embedding = match embedding_api.generate_embedding(issue_text).await {
                            Ok(embedding) => embedding,
                            Err(err) => {
                                error!(issue_number = issue.number, err = err.to_string(), "generate embedding error");
                                continue;
                            }
                        };
                        let embedding =
                            Vector::from(raw_embedding);
                        let issue_id: Option<i32> = match sqlx::query_scalar!(
                            "select id from issues where source_id = $1",
                            issue.id.to_string()
                        )
                        .fetch_optional(&pool)
                        .await {
                            Ok(id) => id,
                            Err(err) => {
                                error!(issue_number = issue.number, err = err.to_string(), "failed to fetch issue id");
                                continue;
                            }
                        };
                        let issue_id = if let Some(id) = issue_id {
                            id
                        } else {
                            match sqlx::query_scalar(
                            r#"insert into issues (source_id, source, title, body, is_pull_request, number, html_url, url, embedding)
                               values ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                               returning id"#
                            )
                            .bind(issue.id.to_string())
                            .bind(source)
                            .bind(issue.title)
                            .bind(issue.body)
                            .bind(issue.is_pull_request)
                            .bind(issue.number)
                            .bind(issue.html_url)
                            .bind(issue.url)
                            .bind(embedding)
                            .fetch_one(&pool)
                            .await {
                                Ok(id) => id,
                                Err(err) => {
                                    error!(issue_number = issue.number, err = err.to_string(), "error inserting issue");
                                    continue;
                                }
                            }
                        };
                        if !issue.comments.is_empty() {
                            let mut qb = QueryBuilder::new(
                                "insert into comments (source_id, body, url, issue_id)",
                            );
                            qb.push_values(issue.comments, |mut b, comment| {
                                b.push_bind(comment.id)
                                    .push_bind(comment.body)
                                    .push_bind(comment.url)
                                    .push_bind(issue_id);
                            });
                            qb.push("on conflict do nothing");
                            if let Err(err) = qb.build().execute(&pool).await {
                                error!(issue_number = issue.number, err = err.to_string(), "error inserting comments");
                            }
                        }
                        if let Some(page) = page {
                            if let Err(err) = sqlx::query(
                                r#"insert into jobs (repository_id, data)
                               values ($1, jsonb_build_object('issues_page', $2))
                               on conflict (repository_id)
                               do update
                               set
                                   data = jsonb_set(jobs.data, '{issues_page}', to_jsonb($2::int), true),
                                   updated_at = current_timestamp"#,
                            )
                            .bind(&repository.repo_id)
                            .bind(page)
                            .execute(&pool)
                            .await {
                                error!(issue_number = issue.number, err = err.to_string(), "error inserting job")
                            }
                        }
                    }
                    if let Err(err) = sqlx::query!(
                        "delete from jobs where repository_id = $1",
                        repository.repo_id
                    )
                    .execute(&pool)
                    .await {
                        error!(err = err.to_string(), "failed to delete job");
                        return;
                    }
                    info!("finished indexing");
                }.instrument(span));
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
               set embedding = $1, updated_at = current_timestamp
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

pub static PRE_SHUTDOWN: AtomicBool = AtomicBool::new(false);

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("Received termination signal shutting down");

    PRE_SHUTDOWN.store(true, Ordering::SeqCst);
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

    let embedding_api = EmbeddingApi::new(config.embedding_api)?;
    let github_api = GithubApi::new(config.github_api, config.message_config.clone())?;
    let huggingface_api = HuggingfaceApi::new(config.huggingface_api, config.message_config)?;
    let slack = Slack::new(&config.slack)?;
    let summarization_api = SummarizationApi::new(config.summarization_api)?;

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
        handle_webhooks_wrapper(
            rx,
            embedding_api,
            github_api,
            huggingface_api,
            slack,
            summarization_api,
            pool
        )
    )?;

    Ok(())
}
