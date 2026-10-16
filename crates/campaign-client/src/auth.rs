//! Client for the auth service: setting up devices and logging in.

use campaign_auth::{
    DeviceCert, DeviceKey, PublicKey, Purpose, Role, SignedCert,
    protocol::{
        Campaign, Challenge, ChallengeRequest, Device, LoginProof, LoginRequest, ServerKey,
        Session, SetupCode, SetupRequest,
    },
};
use uuid::Uuid;

use crate::{ClientError, check, send};

#[derive(Debug, Clone)]
pub struct AuthClient {
    base_url: String,
    http: reqwest::Client,
}

impl AuthClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            http: crate::http_client(None),
        }
    }

    /// For the calls that need a logged-in device (see [`Session::token`]).
    pub fn with_token(mut self, token: &str) -> Self {
        self.http = crate::http_client(Some(token));
        self
    }

    /// The key the server signs tokens with. Clients remember it the first
    /// time and check it on every login after.
    pub async fn server_key(&self) -> Result<PublicKey, ClientError> {
        let key: ServerKey = send(self.http.get(self.url("/public-key"))).await?;
        Ok(key.key)
    }

    /// Makes `key` the DM of the campaign a one-time setup code is for.
    pub async fn setup(
        &self,
        code: &str,
        key: &DeviceKey,
        name: &str,
    ) -> Result<Campaign, ClientError> {
        let code = SetupCode {
            code: code.trim().to_string(),
        };
        let campaign: Campaign =
            send(self.http.post(self.url("/setup/lookup")).json(&code)).await?;
        let cert = DeviceCert::self_signed(key, campaign.id, Role::Dm, name)
            .map_err(|e| ClientError::Rejected(e.to_string()))?;
        let request = SetupRequest {
            code: code.code,
            cert,
        };
        send(self.http.post(self.url("/setup")).json(&request)).await
    }

    /// Answers a login challenge with `key`, returning a short-lived token.
    /// With `expected_server`, refuses to sign for any other server.
    pub async fn login(
        &self,
        campaign: Uuid,
        key: &DeviceKey,
        expected_server: Option<PublicKey>,
    ) -> Result<Session, ClientError> {
        let request = ChallengeRequest {
            campaign,
            key: key.public(),
        };
        let challenge: Challenge =
            send(self.http.post(self.url("/login/challenge")).json(&request)).await?;
        if challenge.campaign != campaign
            || expected_server.is_some_and(|expected| expected != challenge.server)
        {
            return Err(ClientError::Unauthorized(
                "the server's key is not the one this device knows; refusing to log in".into(),
            ));
        }
        let signature = key
            .sign(Purpose::Login, &LoginProof::from(&challenge))
            .map_err(|e| ClientError::Rejected(e.to_string()))?;
        let answer = LoginRequest {
            challenge: challenge.id,
            signature,
        };
        send(self.http.post(self.url("/login")).json(&answer)).await
    }

    pub async fn campaign(&self) -> Result<Campaign, ClientError> {
        send(self.http.get(self.url("/campaign"))).await
    }

    pub async fn devices(&self) -> Result<Vec<Device>, ClientError> {
        send(self.http.get(self.url("/devices"))).await
    }

    /// Adds a device, vouched for by a certificate signed with the DM key
    /// this client is logged in with.
    pub async fn add_device(&self, cert: &SignedCert) -> Result<Device, ClientError> {
        send(self.http.post(self.url("/devices")).json(cert)).await
    }

    pub async fn revoke(&self, key: PublicKey) -> Result<Device, ClientError> {
        let url = self.url(&format!("/devices/{key}"));
        let response = check(self.http.delete(url).send().await?).await?;
        response.json().await.map_err(ClientError::InvalidResponse)
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }
}
