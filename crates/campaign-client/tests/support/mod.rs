//! A real auth service for end-to-end tests, and campaigns with logged-in
//! devices. Needs Postgres (see README); `TEST_POSTGRES_URL` points at the
//! server, and each service gets its own test database on it.

#![allow(dead_code)] // each test file uses a different part

use campaign_auth::{
    DeviceCert, DeviceKey, PublicKey, Role,
    protocol::{Campaign, Session},
};
use campaign_client::auth::AuthClient;
use uuid::Uuid;

pub fn database_url(name: &str) -> String {
    let server = std::env::var("TEST_POSTGRES_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:5432".to_string());
    format!("{}/{name}", server.trim_end_matches('/'))
}

pub async fn serve(app: axum::Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(axum::serve(listener, app).into_future());
    format!("http://{addr}")
}

pub struct Auth {
    pub url: String,
    /// The key its tokens are signed with.
    pub key: PublicKey,
    pub store: auth_service::Store,
}

impl Auth {
    pub async fn start() -> Self {
        let pool = auth_service::connect(&database_url("auth_test"))
            .await
            .expect("Postgres must be running for these tests (see README)");
        let store = auth_service::Store::new(pool.clone());
        let url = serve(auth_service::app(pool).await.unwrap()).await;
        let key = AuthClient::new(&url).server_key().await.unwrap();
        Self { url, key, store }
    }

    pub fn client(&self) -> AuthClient {
        AuthClient::new(&self.url)
    }

    pub async fn login(&self, campaign: Uuid, key: &DeviceKey) -> Session {
        self.client()
            .login(campaign, key, Some(self.key))
            .await
            .unwrap()
    }

    /// A new campaign whose DM device ("DM") is set up and logged in, the
    /// way a DM would: with the setup code printed by `new-campaign`.
    pub async fn campaign(&self) -> Party {
        let campaign = self.store.create_campaign("Test campaign").await.unwrap();
        let code = self
            .store
            .new_setup_code(campaign.id, chrono::Duration::hours(1))
            .await
            .unwrap();
        let dm_key = DeviceKey::generate();
        let joined = self.client().setup(&code, &dm_key, "DM").await.unwrap();
        assert_eq!(joined, campaign);
        let dm = self.login(campaign.id, &dm_key).await;
        Party {
            campaign,
            dm_key,
            dm,
        }
    }
}

pub struct Party {
    pub campaign: Campaign,
    pub dm_key: DeviceKey,
    pub dm: Session,
}

impl Party {
    pub fn id(&self) -> Uuid {
        self.campaign.id
    }

    /// The DM admits a new device, which then logs in.
    pub async fn join(&self, auth: &Auth, name: &str, role: Role) -> (DeviceKey, Session) {
        let key = DeviceKey::generate();
        let cert = DeviceCert::issue(&self.dm_key, self.id(), key.public(), role, name).unwrap();
        auth.client()
            .with_token(&self.dm.token)
            .add_device(&cert)
            .await
            .unwrap();
        let session = auth.login(self.id(), &key).await;
        (key, session)
    }

    pub async fn player(&self, auth: &Auth, name: &str) -> Session {
        self.join(auth, name, Role::Player).await.1
    }
}
