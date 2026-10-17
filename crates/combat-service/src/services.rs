//! Reaching the other services as this service itself: it logs in to the
//! auth service with its own key (registered there with `auth-service
//! add-service`) to get a token for each campaign it works for. Character
//! sheets are kept in step through it (see [`crate::sync`]), and monsters'
//! stat blocks fetched, to add them to a fight.

use std::{collections::HashMap, sync::Arc};

use chrono::Utc;
use edda_auth::{DeviceKey, PublicKey, protocol::Session};
use edda_client::{ClientError, auth::AuthClient, monsters::MonsterClient};
use edda_types::monster::Monster;
use tokio::sync::Mutex;
use uuid::Uuid;

/// Which other services to use, and how to log in to them.
#[derive(Debug, Clone)]
pub struct ServicesConfig {
    pub auth_service: String,
    /// The auth service's token key, checked when logging in.
    pub auth_key: PublicKey,
    /// This service's own key.
    pub key: Arc<DeviceKey>,
    /// Where character sheets are kept in step with combats: hit points
    /// written back, slots and feature uses spent. `None` leaves sheets
    /// alone.
    pub character_service: Option<String>,
    /// Where monsters' stat blocks come from, to add monsters to a fight.
    /// `None` refuses `AddMonsters` and fights from plans.
    pub monster_service: Option<String>,
}

/// Logged in to each campaign this service works for.
pub struct ServiceLogin {
    auth: AuthClient,
    auth_key: PublicKey,
    key: Arc<DeviceKey>,
    /// A token per campaign, reused until shortly before it expires.
    sessions: Mutex<HashMap<Uuid, Session>>,
}

impl ServiceLogin {
    /// Logs in as configured, when a token is first needed.
    pub fn new(config: &ServicesConfig) -> Self {
        Self {
            auth: AuthClient::new(&config.auth_service),
            auth_key: config.auth_key,
            key: config.key.clone(),
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// A token for `campaign`.
    pub async fn token(&self, campaign: Uuid) -> Result<String, ClientError> {
        let mut sessions = self.sessions.lock().await;
        let fresh = |s: &Session| s.claims.expires_at > Utc::now() + chrono::Duration::minutes(1);
        if let Some(session) = sessions.get(&campaign).filter(|s| fresh(s)) {
            return Ok(session.token.clone());
        }
        let session = self
            .auth
            .login(campaign, &self.key, Some(self.auth_key))
            .await?;
        let token = session.token.clone();
        sessions.insert(campaign, session);
        Ok(token)
    }
}

/// The campaigns' monsters, from the monster service.
pub struct Monsters {
    login: Arc<ServiceLogin>,
    url: String,
}

impl Monsters {
    /// The monster service at `url`, logged in to with `login`.
    pub fn new(login: Arc<ServiceLogin>, url: String) -> Self {
        Self { login, url }
    }

    /// A monster `campaign` can use: one of its own or a shared one.
    pub async fn get(&self, campaign: Uuid, id: Uuid) -> Result<Monster, ClientError> {
        let token = self.login.token(campaign).await?;
        MonsterClient::new(&self.url)
            .with_token(&token)
            .get(id)
            .await
    }
}
