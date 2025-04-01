use std::future::ready;

use axum::{http::StatusCode, response::IntoResponse, routing::get, Router};
use metrics_exporter_prometheus::PrometheusHandle;
use tokio::net::TcpListener;
use tracing::info;

use crate::shutdown_signal;

fn metrics_app(recorder_handle: PrometheusHandle, health: bool) -> Router {
    let mut router = Router::new().route("/metrics", get(move || ready(recorder_handle.render())));
    if health {
        router = router.route("/health", get(|| ready(StatusCode::OK.into_response())));
    }

    router
}

pub async fn start_metrics_server(
    ip: String,
    port: u16,
    health: bool,
    recorder_handle: PrometheusHandle,
) -> anyhow::Result<()> {
    let app = metrics_app(recorder_handle, health);

    info!(ip, port, "starting metrics server");
    let listener = TcpListener::bind(format!("{}:{}", ip, port)).await?;
    axum::serve(listener, app.into_make_service())
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}
