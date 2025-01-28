use std::time::Instant;

use axum::{
    extract::{MatchedPath, Request},
    http::HeaderValue,
    middleware::Next,
    response::IntoResponse,
};
use nanoid::nanoid;

pub async fn track_metrics(req: Request, next: Next) -> impl IntoResponse {
    let start = Instant::now();
    let path = if let Some(matched_path) = req.extensions().get::<MatchedPath>() {
        matched_path.as_str().to_owned()
    } else {
        req.uri().path().to_owned()
    };
    let method = req.method().clone();

    let response = next.run(req).await;

    let latency = start.elapsed().as_secs_f64();
    let status = response.status().as_u16().to_string();

    let labels = [
        ("method", method.to_string()),
        ("path", path),
        ("status", status),
    ];

    metrics::histogram!("issue_bot_api_response_time_hist", &labels).record(latency);

    response
}

pub const X_REQUEST_ID: &str = "X-Request-Id";

#[derive(Clone, Debug)]
pub struct RequestId(pub String);

impl RequestId {
    pub fn new() -> Self {
        Self(nanoid!())
    }
}

impl Default for RequestId {
    fn default() -> Self {
        Self::new()
    }
}

pub async fn add_request_id(mut req: Request, next: Next) -> impl IntoResponse {
    let request_id: String = req
        .headers()
        .get(X_REQUEST_ID)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string())
        .unwrap_or_else(|| nanoid!());
    req.extensions_mut().insert(RequestId(request_id.clone()));
    let mut res = next.run(req).await;
    res.headers_mut()
        .insert(X_REQUEST_ID, HeaderValue::from_str(&request_id).unwrap());
    res
}

#[derive(Clone)]
pub struct RequestSpan;

impl<B> tower_http::trace::MakeSpan<B> for RequestSpan {
    fn make_span(&mut self, req: &Request<B>) -> tracing::Span {
        let request_id = req.extensions().get::<RequestId>().unwrap();
        tracing::info_span!("request", request_id = request_id.0.to_string(), method = %req.method(), path = req.uri().path(), uri = %req.uri(),)
    }
}
