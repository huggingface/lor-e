use std::{env, sync::Once, time::Duration};

use axum::{
    error_handling::HandleErrorLayer,
    http::{Response, StatusCode},
    middleware,
    response::IntoResponse,
    routing::get,
    Router,
};
use config::{load_config, IssueBotConfig, ServerConfig};
use metrics::start_metrics_server;
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use middlewares::RequestSpan;
use sqlx::{
    postgres::{PgConnectOptions, PgPoolOptions},
    Pool, Postgres,
};
use tokio::{net::TcpListener, task::JoinHandle};
use tower::{BoxError, ServiceBuilder};
use tower_http::trace::TraceLayer;
use tracing::{info, Span};
use tracing_subscriber::EnvFilter;

mod config;
mod errors;
mod metrics;
mod middlewares;
mod routes;

#[derive(Clone)]
pub struct AppState {
    auth_token: String,
    pool: Pool<Postgres>,
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_logging();

    let config: IssueBotConfig = load_config("ISSUE_BOT")?;

    let opts: PgConnectOptions = config.database.connection_string.parse()?;
    let pool = PgPoolOptions::new()
        .max_connections(config.database.max_connections)
        .connect_with(opts)
        .await?;

    let state = AppState {
        auth_token: config.auth_token,
        pool,
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
        )))
    )?;

    Ok(())
}
