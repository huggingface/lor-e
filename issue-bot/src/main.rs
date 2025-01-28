use std::{env, sync::Once, time::Duration};

use axum::{
    error_handling::HandleErrorLayer,
    http::{Response, StatusCode},
    middleware,
    response::IntoResponse,
    routing::get,
    Router,
};
use config::{load_config, IssueBotConfig};
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use middlewares::RequestSpan;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use tokio::net::TcpListener;
use tower::{BoxError, ServiceBuilder};
use tower_http::trace::TraceLayer;
use tracing::{info, Span};
use tracing_subscriber::EnvFilter;

mod config;
mod errors;
mod middlewares;
mod routes;

fn setup_metrics_recorder() -> PrometheusHandle {
    const EXPONENTIAL_SECONDS: &[f64] = &[
        0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
    ];

    PrometheusBuilder::new()
        .set_buckets_for_metric(
            Matcher::Full("repository_scanner_api_response_time_hist".to_string()),
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_logging();

    let config: IssueBotConfig = load_config("ISSUE_BOT")?;

    let opts = PgConnectOptions::new()
        .host(&config.database.host)
        .password(&config.database.password)
        .port(config.database.port)
        .username(&config.database.user);
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect_with(opts)
        .await?;

    let app = Router::new()
        .nest("/event", routes::event_router())
        .nest("/metrics", routes::metrics_router(setup_metrics_recorder()))
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
        .with_state(pool);

    let addr = format!("{}:{}", config.server.ip, config.server.port);
    info!(addr, "starting server");

    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, app.into_make_service()).await?;

    Ok(())
}
