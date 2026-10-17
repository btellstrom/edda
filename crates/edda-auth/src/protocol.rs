//! Requests and responses of the auth service, and what tokens say.

use chrono::{DateTime, Utc};
use edda_types::Edition;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{Nonce, PublicKey, Role, Signature, SignedCert, bytes};

/// What a token grants: who is calling, in which campaign, as what.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Claims {
    /// The device's (or service's) key.
    pub key: PublicKey,
    pub campaign: Uuid,
    pub role: Principal,
    /// "Vex's laptop", or the service's name. Used as the actor in logs.
    pub name: String,
    /// The rules the campaign plays by, so services can apply them.
    #[serde(default)]
    pub edition: Edition,
    pub expires_at: DateTime<Utc>,
}

/// Who a token is for: a DM device, a player's, or a service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Principal {
    Dm,
    Player,
    /// Another service acting on its own behalf (e.g. the combat service
    /// writing hit points back to character sheets).
    Service,
}

impl From<Role> for Principal {
    fn from(role: Role) -> Self {
        match role {
            Role::Dm => Principal::Dm,
            Role::Player => Principal::Player,
        }
    }
}

impl Claims {
    /// Whether the token is a DM device's.
    pub fn is_dm(&self) -> bool {
        self.role == Principal::Dm
    }
}

/// `POST /login/challenge`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ChallengeRequest {
    pub campaign: Uuid,
    pub key: PublicKey,
}

/// `POST /login/challenge`: what to sign to log in, valid for a minute.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Challenge {
    pub id: Uuid,
    pub nonce: Nonce,
    pub campaign: Uuid,
    /// The auth service's own key, which the device should check against
    /// the one it expects (so it isn't logging in somewhere else).
    pub server: PublicKey,
    pub expires_at: DateTime<Utc>,
}

/// What a device signs to answer a challenge. Covers everything that
/// identifies this login, so the signature is useless anywhere else.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LoginProof {
    pub challenge: Uuid,
    pub nonce: Nonce,
    pub campaign: Uuid,
    pub server: PublicKey,
}

impl From<&Challenge> for LoginProof {
    fn from(c: &Challenge) -> Self {
        Self {
            challenge: c.id,
            nonce: c.nonce,
            campaign: c.campaign,
            server: c.server,
        }
    }
}

/// `POST /login`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LoginRequest {
    pub challenge: Uuid,
    pub signature: Signature,
}

/// `POST /login`: a token, and what it says.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Session {
    /// Send as `Authorization: Bearer <token>`.
    pub token: String,
    pub claims: Claims,
}

/// `POST /setup/lookup`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SetupCode {
    pub code: String,
}

/// `POST /setup`: the first DM device of a campaign, with the one-time code
/// printed when the campaign was created.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SetupRequest {
    pub code: String,
    /// Self-signed, role DM.
    pub cert: SignedCert,
}

/// A campaign: its id, name and edition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Campaign {
    pub id: Uuid,
    pub name: String,
    /// Chosen when the campaign is created.
    #[serde(default)]
    pub edition: Edition,
}

/// A device of a campaign: its certificate, and when it was added and
/// revoked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Device {
    pub cert: SignedCert,
    pub added_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<DateTime<Utc>>,
}

/// `GET /public-key`: the key tokens are signed with.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ServerKey {
    pub key: PublicKey,
}

/// `POST /pairings` (DM only): a slot for pairing a new device.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PairingSlot {
    pub slot: u16,
    pub expires_at: DateTime<Utc>,
}

/// A message relayed between the two sides of a pairing:
/// `PUT`/`GET /pairings/{slot}/{host|joiner}/{n}`. Opaque to the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PairingMessage {
    /// Unpadded base64url.
    #[serde(with = "bytes::vec")]
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub data: Vec<u8>,
}
