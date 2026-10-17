#![allow(clippy::unwrap_used)] // tests panic on failure, which is the point

//! A real auth service for end-to-end tests, and campaigns with logged-in
//! devices. Needs a throwaway Postgres at `TEST_POSTGRES_URL` (`make test`
//! starts one); each service gets its own test database on it.

#![allow(dead_code)] // each test file uses a different part

use edda_auth::{
    DeviceCert, DeviceKey, PublicKey, Role,
    protocol::{Campaign, Session},
};
use edda_client::auth::AuthClient;
use edda_server::Limits;
use uuid::Uuid;

pub use edda_server::testing::database_url;

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
        Self::start_with(Limits::off()).await
    }

    pub async fn start_with(limits: Limits) -> Self {
        let pool = auth_service::connect(&database_url("auth_test").await)
            .await
            .expect("Postgres must be running for these tests (make test starts one; see docs/development.md)");
        let store = auth_service::Store::new(pool.clone());
        let url = serve(auth_service::app(pool, limits).await.unwrap()).await;
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
        let campaign = self
            .store
            .create_campaign("Test campaign", Default::default())
            .await
            .unwrap();
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
