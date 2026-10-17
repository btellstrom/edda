//! What a service measures, for Prometheus: its requests by route and
//! status and how long they take, what the limits refuse, and its
//! database connections; services add their own (the combat service's
//! WebSockets, say). Served at `/metrics` on `METRICS_ADDR`, an address of
//! its own that the proxy never serves; without it nothing is measured.

use std::{sync::OnceLock, time::Instant};

use axum::{
    Router,
    extract::{MatchedPath, Request},
    middleware::{self, Next},
    response::Response,
    routing::get,
};
use metrics_exporter_prometheus::{BuildError, Matcher, PrometheusBuilder, PrometheusHandle};
use sqlx::PgPool;

/// How long requests take, in seconds: the histogram's buckets.
const DURATION_BUCKETS: [f64; 10] = [0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 10.0];

/// The database whose connections are measured, from [`watch_pool`].
static POOL: OnceLock<PgPool> = OnceLock::new();

/// Measures `service` from now on, everything labelled with its name; the
/// handle renders what was measured. Once per process.
pub fn install(service: &str) -> Result<PrometheusHandle, BuildError> {
    builder(service)?.install_recorder()
}

fn builder(service: &str) -> Result<PrometheusBuilder, BuildError> {
    PrometheusBuilder::new()
        .add_global_label("service", service)
        .set_buckets_for_metric(
            Matcher::Full("edda_http_request_duration_seconds".into()),
            &DURATION_BUCKETS,
        )
}

/// `GET /metrics`: what was measured, in Prometheus' text format.
pub fn endpoint(handle: PrometheusHandle) -> Router {
    Router::new().route(
        "/metrics",
        get(move || {
            let handle = handle.clone();
            async move {
                measure_pool();
                handle.run_upkeep();
                handle.render()
            }
        }),
    )
}

/// Counts and times `router`'s requests, by the route they matched (its
/// template, such as `/characters/{id}`, not the ids themselves).
pub fn requests(router: Router) -> Router {
    router.route_layer(middleware::from_fn(time))
}

async fn time(request: Request, next: Next) -> Response {
    let started = Instant::now();
    let method = request.method().to_string();
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or_else(String::new, |path| path.as_str().to_owned());
    let response = next.run(request).await;
    let status = response.status().as_u16().to_string();
    metrics::counter!(
        "edda_http_requests_total",
        "method" => method.clone(),
        "route" => route.clone(),
        "status" => status,
    )
    .increment(1);
    metrics::histogram!(
        "edda_http_request_duration_seconds",
        "method" => method,
        "route" => route,
    )
    .record(started.elapsed().as_secs_f64());
    response
}

/// Counts a request a limit refused: `limit` is which.
pub fn refused(limit: &str) {
    metrics::counter!("edda_limits_refused_total", "limit" => limit.to_owned()).increment(1);
}

/// Measures `pool`'s connections whenever the measurements are read. The
/// first pool a service watches is the one measured.
pub fn watch_pool(pool: &PgPool) {
    let _ = POOL.set(pool.clone());
}

fn measure_pool() {
    let Some(pool) = POOL.get() else { return };
    let idle = pool.num_idle();
    let open = usize::try_from(pool.size()).unwrap_or(usize::MAX);
    gauge_of("edda_db_connections", "idle", idle);
    gauge_of("edda_db_connections", "busy", open.saturating_sub(idle));
}

fn gauge_of(name: &'static str, state: &'static str, count: usize) {
    metrics::gauge!(name, "state" => state).set(as_f64(count));
}

/// `count` as a gauge's value, at most `u32::MAX`: far beyond any count here.
pub fn as_f64(count: usize) -> f64 {
    u32::try_from(count).map_or(f64::from(u32::MAX), f64::from)
}

/// The recorder for this crate's tests, installed once for the process.
#[cfg(test)]
fn test_handle() -> PrometheusHandle {
    static HANDLE: std::sync::Mutex<Option<PrometheusHandle>> = std::sync::Mutex::new(None);
    HANDLE
        .lock()
        .unwrap()
        .get_or_insert_with(|| builder("test").unwrap().install_recorder().unwrap())
        .clone()
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, http::StatusCode};
    use http_body_util::BodyExt;
    use sqlx::postgres::PgPoolOptions;
    use tower::ServiceExt;

    use super::*;

    async fn read(endpoint: &Router) -> String {
        let response = endpoint
            .clone()
            .oneshot(Request::get("/metrics").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn requests_refusals_and_connections_are_measured() {
        let endpoint = endpoint(test_handle());
        let app = requests(Router::new().route("/characters/{id}", get(|| async { "ok" })));
        for id in ["grog", "vex"] {
            let request = Request::get(format!("/characters/{id}")).body(Body::empty());
            app.clone().oneshot(request.unwrap()).await.unwrap();
        }
        refused("sign-ins");
        watch_pool(
            &PgPoolOptions::new()
                .connect_lazy("postgres://postgres@127.0.0.1:1/nothing")
                .unwrap(),
        );

        let measured = read(&endpoint).await;
        let line = |start: &str| {
            measured
                .lines()
                .find(|l| l.starts_with(start))
                .map(str::to_owned)
        };
        let requests = line("edda_http_requests_total{").unwrap();
        assert!(
            requests.contains(r#"route="/characters/{id}""#),
            "{requests}"
        );
        assert!(
            requests.contains(r#"status="200""#) && requests.ends_with(" 2"),
            "{requests}"
        );
        assert!(requests.contains(r#"service="test""#), "{requests}");
        assert!(!measured.contains("grog"), "routes, not ids");
        assert!(line("edda_http_request_duration_seconds_bucket{").is_some());
        assert!(
            line("edda_limits_refused_total{")
                .unwrap()
                .contains(r#"limit="sign-ins""#)
        );
        assert!(line("edda_db_connections{").is_some(), "{measured}");
    }

    #[test]
    fn counts_become_gauge_values() {
        assert!((as_f64(3) - 3.0).abs() < f64::EPSILON);
        assert!((as_f64(usize::MAX) - f64::from(u32::MAX)).abs() < f64::EPSILON);
    }
}
