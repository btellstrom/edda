//! What a service logs about each request: a span with its method, path,
//! request id, and whose it is (the campaign, once its token is checked,
//! and the account, as the proxy says), in plain text or JSON lines. Never
//! a body, and never the query string, which may hold what someone typed.

use axum::{
    Router,
    extract::Request,
    http::{HeaderName, HeaderValue},
    middleware::{self, Next},
    response::Response,
};
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    trace::{DefaultOnResponse, TraceLayer},
};
use tracing::{Level, Span, Subscriber, field};
use tracing_subscriber::{EnvFilter, fmt::MakeWriter};

/// The request's id: the proxy's, or one of the service's own. Answered in
/// the response too, so a user's report can be found in the logs.
pub const REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

/// The account a request is for, set by the hosting's proxy (from the
/// address it came to). Believed only with `TRUST_PROXY`.
pub const ACCOUNT: HeaderName = HeaderName::from_static("x-edda-account");

/// Logs `router`'s requests. With `trust_proxy`, the proxy's request id
/// and account are believed; without, a client's are dropped.
pub fn requests(router: Router, trust_proxy: bool) -> Router {
    router
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(move |request: &Request| span(request, trust_proxy))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .layer(PropagateRequestIdLayer::new(REQUEST_ID))
        .layer(SetRequestIdLayer::new(REQUEST_ID, MakeRequestUuid))
        .layer(middleware::from_fn(move |request, next| {
            unless_trusted(request, next, trust_proxy)
        }))
}

/// Drops the headers only the proxy may set, unless it's trusted.
async fn unless_trusted(mut request: Request, next: Next, trust_proxy: bool) -> Response {
    if !trust_proxy {
        request.headers_mut().remove(REQUEST_ID);
        request.headers_mut().remove(ACCOUNT);
    }
    next.run(request).await
}

fn span(request: &Request, trust_proxy: bool) -> Span {
    let header = |name| {
        request
            .headers()
            .get(name)
            .and_then(|v: &HeaderValue| v.to_str().ok())
            .map(str::to_owned)
    };
    let account = if trust_proxy { header(ACCOUNT) } else { None };
    tracing::info_span!(
        "request",
        method = %request.method(),
        path = request.uri().path(),
        request_id = header(REQUEST_ID),
        account,
        campaign = field::Empty,
    )
}

/// The subscriber `init_logging` installs, writing to `writer`: JSON lines
/// when `json`, otherwise text for people.
pub fn subscriber<W>(json: bool, filter: EnvFilter, writer: W) -> Box<dyn Subscriber + Send + Sync>
where
    W: for<'w> MakeWriter<'w> + Send + Sync + 'static,
{
    let builder = tracing_subscriber::fmt()
        .with_writer(writer)
        .with_env_filter(filter);
    if json {
        Box::new(
            builder
                .json()
                .with_current_span(true)
                .with_span_list(false)
                .finish(),
        )
    } else {
        Box::new(builder.finish())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io,
        sync::{Arc, Mutex},
    };

    use axum::{body::Body, routing::get};
    use serde_json::Value;
    use tower::ServiceExt;

    use super::*;

    /// What was logged.
    #[derive(Clone, Default)]
    struct Logged(Arc<Mutex<Vec<u8>>>);

    impl io::Write for Logged {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'w> MakeWriter<'w> for Logged {
        type Writer = Logged;

        fn make_writer(&'w self) -> Self::Writer {
            self.clone()
        }
    }

    impl Logged {
        /// The spans the lines were logged in.
        fn spans(&self) -> Vec<Value> {
            let text = String::from_utf8(self.0.lock().unwrap().clone()).unwrap();
            text.lines()
                .map(|line| serde_json::from_str::<Value>(line).unwrap()["span"].clone())
                .filter(|span| !span.is_null())
                .collect()
        }

        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    /// Sends `request` through a service whose handler records campaign
    /// `c-1`, as checking a token does; returns its response's request id.
    async fn send(request: Request, trust_proxy: bool, logged: &Logged) -> String {
        let _logging = tracing::subscriber::set_default(subscriber(
            true,
            EnvFilter::new("info"),
            logged.clone(),
        ));
        let handler = || async {
            Span::current().record("campaign", "c-1");
            "ok"
        };
        let response = requests(
            Router::new().route("/characters", get(handler)),
            trust_proxy,
        )
        .oneshot(request)
        .await
        .unwrap();
        response.headers()[REQUEST_ID].to_str().unwrap().to_owned()
    }

    fn request() -> Request {
        Request::get("/characters?search=Secret%20Plans")
            .header(REQUEST_ID, "from-the-proxy")
            .header(ACCOUNT, "vox")
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn requests_are_logged_with_whose_they_are_and_never_what_was_asked() {
        let logged = Logged::default();
        let id = send(request(), true, &logged).await;
        assert_eq!(id, "from-the-proxy");
        let spans = logged.spans();
        assert!(!spans.is_empty(), "{}", logged.text());
        for span in spans {
            assert_eq!(span["path"], "/characters");
            assert_eq!(span["request_id"], "from-the-proxy");
            assert_eq!(span["account"], "vox");
        }
        assert!(logged.text().contains(r#""campaign":"c-1""#));
        assert!(!logged.text().contains("Secret"), "the query stays out");
    }

    #[tokio::test]
    async fn without_a_trusted_proxy_clients_cant_say_whose_they_are() {
        let logged = Logged::default();
        let id = send(request(), false, &logged).await;
        assert_ne!(id, "from-the-proxy", "a request id of the service's own");
        for span in logged.spans() {
            assert_eq!(span["request_id"], id.as_str());
            assert!(span.get("account").is_none(), "{span}");
        }
    }
}
