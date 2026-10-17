//! Limits on how much one client or one campaign can ask of a service, so
//! a service open to the internet can't be flooded or filled up: requests
//! per minute from each client address (fewer for signing in), how big a
//! request may be, and how much each campaign may keep.
//!
//! All on by default. `LIMITS=off` turns every one off, for running on
//! your own machine, and `LIMIT_<NAME>=<number>` changes one, such as
//! `LIMIT_CHARACTERS=1000` (the names are the fields below, in capitals).
//!
//! Requests are counted per instance of a service, in memory, by client
//! address. Behind the HTTPS proxy every request comes from the proxy, so
//! `TRUST_PROXY=1` takes the client's address from the `X-Forwarded-For`
//! header the proxy adds instead, and counts only requests that came
//! through it: the services' calls to each other (the combat service's to
//! the character service, say) aren't limited. Only set it where nothing
//! but the proxy and the other services can reach the service, or clients
//! could claim any address, or none.

use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    Json, Router,
    extract::{ConnectInfo, DefaultBodyLimit, Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use serde_json::json;

/// What a service allows; see the module's description.
#[derive(Debug, Clone)]
pub struct Limits {
    enabled: bool,
    /// Requests per minute from one client address, to any route.
    pub requests_per_minute: u32,
    /// Requests per minute from one client address to the routes that
    /// need no token: logging in, setting up a campaign, joining a pairing.
    pub sign_ins_per_minute: u32,
    /// The largest request body accepted.
    pub body_bytes: usize,
    /// What one campaign may keep.
    pub characters: u32,
    /// Its own monsters, and its own spells and its own conditions (each).
    pub monsters: u32,
    pub rules_texts: u32,
    pub combats: u32,
    pub plans: u32,
    pub devices: u32,
    pub events_per_combat: u32,
    /// Devices watching one combat at once (per instance).
    pub connections_per_combat: u32,
    /// Whether a request's client is the last `X-Forwarded-For` address
    /// (`TRUST_PROXY=1`); see the module's description.
    pub trust_proxy: bool,
}

impl Default for Limits {
    /// No limits at all: what tests, and a service on your own machine,
    /// want. A service reads its own with [`Limits::from_env`].
    fn default() -> Self {
        Self::off()
    }
}

impl Limits {
    /// No limits at all.
    pub fn off() -> Self {
        Self {
            enabled: false,
            ..Self::on()
        }
    }

    /// The limits a service open to others starts with.
    pub fn on() -> Self {
        Self {
            enabled: true,
            requests_per_minute: 600,
            sign_ins_per_minute: 20,
            body_bytes: 1024 * 1024,
            characters: 200,
            monsters: 1000,
            rules_texts: 1000,
            combats: 500,
            plans: 200,
            devices: 50,
            events_per_combat: 20_000,
            connections_per_combat: 50,
            trust_proxy: false,
        }
    }

    /// The limits from the environment: on unless `LIMITS=off`, each
    /// changed by `LIMIT_<NAME>`, and `TRUST_PROXY`.
    pub fn from_env() -> Result<Self, String> {
        Self::from_vars(|name| std::env::var(name).ok())
    }

    fn from_vars(var: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let on = match var("LIMITS").as_deref() {
            None | Some("on") => true,
            Some("off") => false,
            Some(other) => return Err(format!("LIMITS must be on or off, not {other:?}")),
        };
        let mut limits = if on { Self::on() } else { Self::off() };
        let number = |name: &str, current: u32| -> Result<u32, String> {
            match var(&format!("LIMIT_{name}")) {
                None => Ok(current),
                Some(value) => value
                    .parse()
                    .map_err(|_| format!("LIMIT_{name} must be a number, not {value:?}")),
            }
        };
        limits.requests_per_minute = number("REQUESTS_PER_MINUTE", limits.requests_per_minute)?;
        limits.sign_ins_per_minute = number("SIGN_INS_PER_MINUTE", limits.sign_ins_per_minute)?;
        let body = u32::try_from(limits.body_bytes).unwrap_or(u32::MAX);
        limits.body_bytes = usize::try_from(number("BODY_BYTES", body)?).unwrap_or(usize::MAX);
        limits.characters = number("CHARACTERS", limits.characters)?;
        limits.monsters = number("MONSTERS", limits.monsters)?;
        limits.rules_texts = number("RULES_TEXTS", limits.rules_texts)?;
        limits.combats = number("COMBATS", limits.combats)?;
        limits.plans = number("PLANS", limits.plans)?;
        limits.devices = number("DEVICES", limits.devices)?;
        limits.events_per_combat = number("EVENTS_PER_COMBAT", limits.events_per_combat)?;
        limits.connections_per_combat =
            number("CONNECTIONS_PER_COMBAT", limits.connections_per_combat)?;
        limits.trust_proxy = matches!(var("TRUST_PROXY").as_deref(), Some("1" | "true" | "on"));
        Ok(limits)
    }

    /// Whether the limits are on (`LIMITS` isn't `off`).
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Whether a campaign that keeps `used` of something may add `more`,
    /// given its limit `max`; if not, what to tell it.
    pub fn room_for(&self, used: i64, more: u32, max: u32, what: &str) -> Result<(), String> {
        if self.enabled && used.saturating_add(i64::from(more)) > i64::from(max) {
            return Err(format!(
                "this campaign has reached its limit of {max} {what}"
            ));
        }
        Ok(())
    }

    /// `router` with the limits on every request: how many a client may
    /// send, and how big. Applied by [`crate::serve`].
    pub(crate) fn apply(&self, router: Router) -> Router {
        if !self.enabled {
            return router;
        }
        let limiter = RateLimiter::new(self.requests_per_minute, self.trust_proxy);
        router
            .layer(middleware::from_fn_with_state(limiter, limit))
            .layer(DefaultBodyLimit::max(self.body_bytes))
    }

    /// `router` (the routes that need no token) with the stricter limit on
    /// signing in.
    pub fn guard_sign_ins<S: Clone + Send + Sync + 'static>(&self, router: Router<S>) -> Router<S> {
        if !self.enabled {
            return router;
        }
        let limiter = RateLimiter::new(self.sign_ins_per_minute, self.trust_proxy);
        router.route_layer(middleware::from_fn_with_state(limiter, limit))
    }
}

/// Counts each client address's requests in one-minute windows.
#[derive(Clone)]
struct RateLimiter {
    per_minute: u32,
    trust_proxy: bool,
    windows: Arc<Mutex<HashMap<IpAddr, (Instant, u32)>>>,
}

const WINDOW: Duration = Duration::from_mins(1);
/// Windows remembered before the finished ones are dropped.
const REMEMBERED: usize = 10_000;

impl RateLimiter {
    fn new(per_minute: u32, trust_proxy: bool) -> Self {
        Self {
            per_minute,
            trust_proxy,
            windows: Arc::default(),
        }
    }

    /// Counts a request from `client`: `Err` with the seconds to wait if
    /// it's one too many.
    fn count(&self, client: IpAddr, now: Instant) -> Result<(), u64> {
        let mut windows = self.windows.lock().expect("not poisoned");
        if windows.len() > REMEMBERED {
            windows.retain(|_, (start, _)| now.duration_since(*start) < WINDOW);
        }
        let (start, count) = windows.entry(client).or_insert((now, 0));
        if now.duration_since(*start) >= WINDOW {
            (*start, *count) = (now, 0);
        }
        if *count >= self.per_minute {
            let left = WINDOW.saturating_sub(now.duration_since(*start));
            return Err(left.as_secs().max(1));
        }
        *count += 1;
        Ok(())
    }

    /// Where `request` comes from, to count it: behind a trusted proxy, its
    /// word for it (none for what didn't come through it); else the
    /// connection's address (unknown when served without it).
    fn client(&self, request: &Request) -> Option<IpAddr> {
        if self.trust_proxy {
            // The proxy adds the address it saw last.
            return request
                .headers()
                .get("x-forwarded-for")?
                .to_str()
                .ok()?
                .rsplit(',')
                .next()?
                .trim()
                .parse()
                .ok();
        }
        request
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(address)| address.ip())
    }
}

async fn limit(State(limiter): State<RateLimiter>, request: Request, next: Next) -> Response {
    if let Some(client) = limiter.client(&request)
        && let Err(wait) = limiter.count(client, Instant::now())
    {
        tracing::debug!(%client, "too many requests");
        let mut response = (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "error": format!("too many requests; try again in {wait} s") })),
        )
            .into_response();
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from(wait));
        return response;
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let pairs: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name| pairs.get(name).cloned()
    }

    #[test]
    fn limits_are_on_unless_turned_off_and_each_can_change() {
        let limits = Limits::from_vars(vars(&[])).unwrap();
        assert!(limits.enabled() && !limits.trust_proxy);
        let limits =
            Limits::from_vars(vars(&[("LIMIT_CHARACTERS", "5"), ("TRUST_PROXY", "1")])).unwrap();
        assert_eq!(limits.characters, 5);
        assert!(limits.trust_proxy);
        assert!(
            !Limits::from_vars(vars(&[("LIMITS", "off")]))
                .unwrap()
                .enabled()
        );
        assert!(Limits::from_vars(vars(&[("LIMITS", "maybe")])).is_err());
        assert!(Limits::from_vars(vars(&[("LIMIT_PLANS", "lots")])).is_err());
    }

    #[test]
    fn quotas_say_when_a_campaign_is_full() {
        let limits = Limits::on();
        assert_eq!(limits.room_for(199, 1, 200, "characters"), Ok(()));
        assert_eq!(
            limits.room_for(200, 1, 200, "characters"),
            Err("this campaign has reached its limit of 200 characters".into())
        );
        assert_eq!(Limits::off().room_for(10_000, 1, 200, "characters"), Ok(()));
    }

    /// A service with `limits`.
    fn service(limits: &Limits) -> Router {
        use axum::routing::post;
        limits.apply(Router::new().route("/", post(|_: String| async { "ok" })))
    }

    /// A request to `app` from `client`, with a body `body` bytes long.
    async fn send(app: &Router, client: &str, body: usize) -> (StatusCode, Option<HeaderValue>) {
        use axum::body::Body;
        use tower::ServiceExt;

        let mut request = Request::post("/")
            .body(Body::from("x".repeat(body)))
            .expect("a valid request");
        let address: SocketAddr = format!("{client}:1234").parse().expect("an address");
        request.extensions_mut().insert(ConnectInfo(address));
        let response = app.clone().oneshot(request).await.expect("infallible");
        (
            response.status(),
            response.headers().get(header::RETRY_AFTER).cloned(),
        )
    }

    #[tokio::test]
    async fn a_client_too_busy_or_too_big_is_turned_away() {
        let mut limits = Limits::on();
        limits.requests_per_minute = 2;
        limits.body_bytes = 100;
        let app = service(&limits);
        assert_eq!(send(&app, "192.0.2.1", 10).await.0, StatusCode::OK);
        assert_eq!(send(&app, "192.0.2.1", 10).await.0, StatusCode::OK);
        let (status, wait) = send(&app, "192.0.2.1", 10).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert!(wait.is_some(), "says when to try again");
        assert_eq!(send(&app, "192.0.2.2", 10).await.0, StatusCode::OK);
        assert_eq!(
            send(&app, "192.0.2.3", 200).await.0,
            StatusCode::PAYLOAD_TOO_LARGE
        );
        // Behind the proxy, the services' calls to each other don't count.
        limits.trust_proxy = true;
        let app = service(&limits);
        for _ in 0..5 {
            assert_eq!(send(&app, "192.0.2.1", 10).await.0, StatusCode::OK);
        }
        // Off, neither.
        let app = service(&Limits::off());
        for _ in 0..5 {
            assert_eq!(send(&app, "192.0.2.1", 200).await.0, StatusCode::OK);
        }
    }

    #[test]
    fn each_address_gets_its_requests_per_minute() {
        let limiter = RateLimiter::new(3, false);
        let (a, b): (IpAddr, IpAddr) = ("192.0.2.1".parse().unwrap(), "192.0.2.2".parse().unwrap());
        let now = Instant::now();
        for _ in 0..3 {
            assert_eq!(limiter.count(a, now), Ok(()));
        }
        assert_eq!(limiter.count(a, now + Duration::from_secs(20)), Err(40));
        assert_eq!(limiter.count(b, now), Ok(()), "another address has its own");
        assert_eq!(limiter.count(a, now + WINDOW), Ok(()), "a new minute");
    }
}
