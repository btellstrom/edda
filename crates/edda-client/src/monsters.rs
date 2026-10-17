//! Client for the monster service: monsters, and the full text of spells
//! and conditions.

use edda_types::{
    monster::{Monster, MonsterSheet, MonsterSummary},
    rules_text::{Entry, RulesText},
};
use uuid::Uuid;

use crate::{ClientError, check, http_client, send};

/// The monster service's client: monsters, spells and conditions.
#[derive(Debug, Clone)]
pub struct MonsterClient {
    base_url: String,
    http: reqwest::Client,
}

impl MonsterClient {
    /// A client for the monster service at `base_url`, without a token.
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

    /// One monster's stat block.
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

    /// Deletes one of the campaign's own monsters.
    pub async fn delete(&self, id: Uuid) -> Result<(), ClientError> {
        let response = self.http.delete(self.monster_url(id)).send().await?;
        check(response).await.map(drop)
    }

    /// The shared spells (`T` = [`edda_types::rules_text::SpellText`])
    /// or conditions of the campaign's edition and its own, whole, by name;
    /// only those whose name contains `search`, if given.
    pub async fn texts<T: RulesText>(
        &self,
        search: Option<&str>,
    ) -> Result<Vec<Entry<T>>, ClientError> {
        let mut request = self.http.get(self.url(&format!("/{}", T::KIND)));
        if let Some(search) = search {
            request = request.query(&[("search", search)]);
        }
        send(request).await
    }

    /// One spell or condition.
    pub async fn text<T: RulesText>(&self, id: Uuid) -> Result<Entry<T>, ClientError> {
        send(self.http.get(self.text_url::<T>(id))).await
    }

    /// Adds one to the campaign's own.
    pub async fn create_text<T: RulesText>(&self, text: &T) -> Result<Entry<T>, ClientError> {
        send(
            self.http
                .post(self.url(&format!("/{}", T::KIND)))
                .json(text),
        )
        .await
    }

    /// Replaces one of the campaign's own ([`ClientError::Forbidden`] for a
    /// shared one).
    pub async fn replace_text<T: RulesText>(
        &self,
        id: Uuid,
        text: &T,
    ) -> Result<Entry<T>, ClientError> {
        send(self.http.put(self.text_url::<T>(id)).json(text)).await
    }

    /// Deletes one of the campaign's own spells or conditions.
    pub async fn delete_text<T: RulesText>(&self, id: Uuid) -> Result<(), ClientError> {
        let response = self.http.delete(self.text_url::<T>(id)).send().await?;
        check(response).await.map(drop)
    }

    fn text_url<T: RulesText>(&self, id: Uuid) -> String {
        self.url(&format!("/{}/{id}", T::KIND))
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    fn monster_url(&self, id: Uuid) -> String {
        self.url(&format!("/monsters/{id}"))
    }
}
