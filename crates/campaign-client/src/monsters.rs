//! Client for the monster service.

use campaign_types::monster::{Monster, MonsterSheet, MonsterSummary};
use uuid::Uuid;

use crate::{ClientError, check, http_client, send};

#[derive(Debug, Clone)]
pub struct MonsterClient {
    base_url: String,
    http: reqwest::Client,
}

impl MonsterClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            http: http_client(None),
        }
    }

    /// Sends `token` (from [`crate::auth::AuthClient::login`]) with every
    /// request.
    pub fn with_token(mut self, token: &str) -> Self {
        self.http = http_client(Some(token));
        self
    }

    /// The shared monsters of the campaign's edition and its own, by name;
    /// only those whose name contains `search`, if given.
    pub async fn list(&self, search: Option<&str>) -> Result<Vec<MonsterSummary>, ClientError> {
        let mut request = self.http.get(self.url("/monsters"));
        if let Some(search) = search {
            request = request.query(&[("search", search)]);
        }
        send(request).await
    }

    pub async fn get(&self, id: Uuid) -> Result<Monster, ClientError> {
        send(self.http.get(self.monster_url(id))).await
    }

    /// Adds a monster to the campaign's own.
    pub async fn create(&self, sheet: &MonsterSheet) -> Result<Monster, ClientError> {
        send(self.http.post(self.url("/monsters")).json(sheet)).await
    }

    /// Replaces one of the campaign's own monsters ([`ClientError::Forbidden`]
    /// for a shared one).
    pub async fn replace(&self, id: Uuid, sheet: &MonsterSheet) -> Result<Monster, ClientError> {
        send(self.http.put(self.monster_url(id)).json(sheet)).await
    }

    pub async fn delete(&self, id: Uuid) -> Result<(), ClientError> {
        let response = self.http.delete(self.monster_url(id)).send().await?;
        check(response).await.map(drop)
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    fn monster_url(&self, id: Uuid) -> String {
        self.url(&format!("/monsters/{id}"))
    }
}
