//! Typed HTTP client for the Edda services. Shared by every
//! frontend so none of them need to know about URLs, headers or status codes.

pub mod auth;
pub mod combat;
pub mod monsters;
pub mod pairing;

use edda_types::{Character, CharacterAction, CharacterSheet, CharacterSummary};
use reqwest::{RequestBuilder, Response, StatusCode, header};
use serde::{Deserialize, de::DeserializeOwned};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("not found")]
    NotFound,
    /// The character changed on the server since this copy was fetched.
    #[error("character was changed by someone else (now at version {})", .0.version)]
    Stale(Box<Character>),
    /// The character is out of the slot/uses the action needed.
    #[error("{0}")]
    Exhausted(String),
    /// The server refused the request as invalid.
    #[error("{0}")]
    Rejected(String),
    #[error("server error ({status}): {message}")]
    Server { status: StatusCode, message: String },
    #[error("could not reach server")]
    Http(#[from] reqwest::Error),
    /// The server answered, but not with what this client expected; usually
    /// a client/server version mismatch.
    #[error("unexpected response from server")]
    InvalidResponse(#[source] reqwest::Error),
    /// No valid credentials: not logged in, or the token expired.
    #[error("not authorized: {0}")]
    Unauthorized(String),
    /// Logged in, but not allowed to do this.
    #[error("forbidden: {0}")]
    Forbidden(String),
    #[error("websocket error: {0}")]
    WebSocket(String),
}

#[derive(Debug, Clone)]
pub struct Client {
    base_url: String,
    http: reqwest::Client,
}

impl Client {
    /// How many times [`Client::modify`] re-applies a change after losing a
    /// race with another writer before giving up.
    pub const MAX_RETRIES: usize = 5;

    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            http: http_client(None),
        }
    }

    /// Sends `token` (from [`auth::AuthClient::login`]) with every request.
    pub fn with_token(mut self, token: &str) -> Self {
        self.http = http_client(Some(token));
        self
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub async fn list(&self) -> Result<Vec<CharacterSummary>, ClientError> {
        send(self.http.get(self.url("/characters"))).await
    }

    pub async fn get(&self, id: Uuid) -> Result<Character, ClientError> {
        send(self.http.get(self.character_url(id))).await
    }

    pub async fn create(&self, sheet: &CharacterSheet) -> Result<Character, ClientError> {
        send(self.http.post(self.url("/characters")).json(sheet)).await
    }

    /// Replaces the sheet, failing with [`ClientError::Stale`] if the server's
    /// copy is no longer at `version`.
    pub async fn replace(
        &self,
        id: Uuid,
        version: u64,
        sheet: &CharacterSheet,
    ) -> Result<Character, ClientError> {
        let request = self
            .http
            .put(self.character_url(id))
            .header(header::IF_MATCH, format!("\"{version}\""))
            .json(sheet);
        send(request).await
    }

    /// Fetches the character, applies `change` and saves it. If someone else
    /// saved in between, `change` is re-applied to their newer copy, so
    /// edits merge instead of overwriting each other. `change` may therefore
    /// run more than once and should only express the caller's intent
    /// ("set max HP to 50"), not depend on the previous value being fixed.
    pub async fn modify<E>(
        &self,
        id: Uuid,
        mut change: impl FnMut(&mut CharacterSheet) -> Result<(), E>,
    ) -> Result<Character, E>
    where
        E: From<ClientError>,
    {
        let mut current = self.get(id).await?;
        for _ in 0..Self::MAX_RETRIES {
            let mut sheet = current.sheet.clone();
            change(&mut sheet)?;
            match self.replace(id, current.version, &sheet).await {
                Err(ClientError::Stale(latest)) => current = *latest,
                result => return Ok(result?),
            }
        }
        Err(ClientError::Stale(Box::new(current)).into())
    }

    pub async fn delete(&self, id: Uuid) -> Result<(), ClientError> {
        let response = self.http.delete(self.character_url(id)).send().await?;
        check(response).await.map(drop)
    }

    /// Sets which player devices play the character, by key (DM devices
    /// only).
    pub async fn set_owners(&self, id: Uuid, owners: &[String]) -> Result<Character, ClientError> {
        let url = format!("{}/owners", self.character_url(id));
        send(self.http.put(url).json(owners)).await
    }

    pub async fn act(&self, id: Uuid, action: &CharacterAction) -> Result<Character, ClientError> {
        let url = format!("{}/actions", self.character_url(id));
        send(self.http.post(url).json(action)).await
    }

    /// Like [`Client::act`], but only once per `key`: sending it again
    /// with the same key changes nothing, so it's safe to retry.
    pub async fn act_once(
        &self,
        id: Uuid,
        action: &CharacterAction,
        key: Uuid,
    ) -> Result<Character, ClientError> {
        let url = format!("{}/actions", self.character_url(id));
        let request = self
            .http
            .post(url)
            .header("Idempotency-Key", key.to_string())
            .json(action);
        send(request).await
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    fn character_url(&self, id: Uuid) -> String {
        self.url(&format!("/characters/{id}"))
    }
}

#[derive(Deserialize)]
struct ErrorBody {
    error: String,
    current: Option<Box<Character>>,
}

/// An HTTP client that sends `token` as a bearer token on every request.
pub(crate) fn http_client(token: Option<&str>) -> reqwest::Client {
    let mut headers = header::HeaderMap::new();
    if let Some(token) = token {
        let mut value = header::HeaderValue::from_str(&format!("Bearer {token}"))
            .expect("tokens are plain ASCII");
        value.set_sensitive(true);
        headers.insert(header::AUTHORIZATION, value);
    }
    reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .expect("HTTP client builds")
}

pub(crate) async fn send<T: DeserializeOwned>(request: RequestBuilder) -> Result<T, ClientError> {
    let response = check(request.send().await?).await?;
    response.json().await.map_err(ClientError::InvalidResponse)
}

/// Turns non-success responses into the matching [`ClientError`].
pub(crate) async fn check(response: Response) -> Result<Response, ClientError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.text().await.unwrap_or_default();
    let (message, current) = match serde_json::from_str::<ErrorBody>(&body) {
        Ok(e) => (e.error, e.current),
        Err(_) => (body, None),
    };
    Err(match status {
        StatusCode::NOT_FOUND => ClientError::NotFound,
        StatusCode::UNAUTHORIZED => ClientError::Unauthorized(message),
        StatusCode::FORBIDDEN => ClientError::Forbidden(message),
        StatusCode::PRECONDITION_FAILED => match current {
            Some(current) => ClientError::Stale(current),
            None => ClientError::Server { status, message },
        },
        StatusCode::CONFLICT => ClientError::Exhausted(message),
        s if s.is_client_error() => ClientError::Rejected(message),
        _ => ClientError::Server { status, message },
    })
}
