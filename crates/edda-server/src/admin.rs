//! The admin API: what whoever runs the services asks of them over HTTP,
//! such as creating and deleting campaigns, so a hosting's control plane
//! can do it without a shell. Served on `ADMIN_ADDR`, an address only it
//! can reach, and only with `ADMIN_TOKEN` too, which every request must
//! bear; without both there's none.

use axum::{
    Router,
    extract::Request,
    http::{StatusCode, header},
    middleware::{self, Next},
    response::Response,
};

/// Where to serve the admin API, and the token it wants.
#[derive(Clone)]
pub struct Admin {
    pub address: String,
    token: String,
}

/// Leaves the token out.
impl std::fmt::Debug for Admin {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("Admin")
            .field("address", &self.address)
            .finish_non_exhaustive()
    }
}

/// The shortest token accepted: guessing it must be hopeless.
const MIN_TOKEN: usize = 32;

impl Admin {
    /// From `ADMIN_ADDR` and `ADMIN_TOKEN`: `None` with neither, an error
    /// with only one, or a token too short to be safe.
    pub fn from_env() -> Result<Option<Self>, String> {
        let var = |name| std::env::var(name).ok().filter(|v: &String| !v.is_empty());
        Self::from(var("ADMIN_ADDR"), var("ADMIN_TOKEN"))
    }

    fn from(address: Option<String>, token: Option<String>) -> Result<Option<Self>, String> {
        match (address, token) {
            (None, None) => Ok(None),
            (Some(_), None) => Err("ADMIN_ADDR needs ADMIN_TOKEN".into()),
            (None, Some(_)) => Err("ADMIN_TOKEN needs ADMIN_ADDR".into()),
            (Some(address), Some(token)) => Self::new(address, token).map(Some),
        }
    }

    /// Serving on `address`, wanting `token`: at least 32 characters, of
    /// printable ASCII (as in a header).
    pub fn new(address: impl Into<String>, token: impl Into<String>) -> Result<Self, String> {
        let token = token.into();
        if token.len() < MIN_TOKEN || !token.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(format!(
                "ADMIN_TOKEN must be at least {MIN_TOKEN} printable ASCII characters"
            ));
        }
        Ok(Self {
            address: address.into(),
            token,
        })
    }

    /// `router`, answering only requests that bear the token.
    pub fn guard(&self, router: Router) -> Router {
        let token = self.token.clone();
        router.layer(middleware::from_fn(move |request, next| {
            let token = token.clone();
            async move { check(&token, request, next).await }
        }))
    }
}

async fn check(token: &str, request: Request, next: Next) -> Response {
    let bearer = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    match bearer {
        Some(given) if same(given.trim().as_bytes(), token.as_bytes()) => next.run(request).await,
        _ => crate::error_response(StatusCode::UNAUTHORIZED, &"the admin token is required"),
    }
}

/// Whether `a` and `b` are the same, taking as long however much of them
/// matches, so the time taken says nothing about the token.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0, |differ, (x, y)| differ | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, routing::get};
    use tower::ServiceExt;

    use super::*;

    const TOKEN: &str = "a-long-enough-admin-token-for-the-tests";

    #[test]
    fn it_takes_both_settings_or_neither() {
        let some = |s: &str| Some(s.to_owned());
        assert!(Admin::from(None, None).unwrap().is_none());
        assert!(Admin::from(some("0.0.0.0:9200"), None).is_err());
        assert!(Admin::from(None, some(TOKEN)).is_err());
        assert!(Admin::from(some("0.0.0.0:9200"), some("short")).is_err());
        let spaced = format!("{TOKEN} with spaces");
        assert!(
            Admin::new("0.0.0.0:9200", spaced).is_err(),
            "it goes in a header"
        );
        let admin = Admin::from(some("0.0.0.0:9200"), some(TOKEN))
            .unwrap()
            .unwrap();
        assert_eq!(admin.address, "0.0.0.0:9200");
        assert!(
            !format!("{admin:?}").contains(TOKEN),
            "the token stays out of logs"
        );
    }

    #[tokio::test]
    async fn only_requests_bearing_the_token_get_through() {
        let admin = Admin::from(Some("unused".into()), Some(TOKEN.into()))
            .unwrap()
            .unwrap();
        let app = admin.guard(Router::new().route("/admin/x", get(|| async { "done" })));
        let status = |authorization: Option<String>| {
            let app = app.clone();
            async move {
                let mut request = Request::get("/admin/x");
                if let Some(value) = authorization {
                    request = request.header(header::AUTHORIZATION, value);
                }
                app.oneshot(request.body(Body::empty()).unwrap())
                    .await
                    .unwrap()
                    .status()
            }
        };
        assert_eq!(status(None).await, StatusCode::UNAUTHORIZED);
        assert_eq!(
            status(Some("Bearer wrong".into())).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status(Some(format!("Bearer {TOKEN}x"))).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status(Some(format!("Bearer {TOKEN}"))).await,
            StatusCode::OK
        );
    }

    #[test]
    fn comparing_is_exact() {
        assert!(same(b"abc", b"abc"));
        assert!(!same(b"abc", b"abd"));
        assert!(!same(b"abc", b"ab"));
    }
}
