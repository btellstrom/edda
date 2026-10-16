//! Persistence in Postgres.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use campaign_auth::{
    DeviceKey, Nonce, PublicKey, Role, SignedCert,
    protocol::{Campaign, Challenge, Device, Principal},
};
use campaign_types::Edition;
use chrono::{DateTime, Duration, Utc};
use sha2::{Digest, Sha256};
use sqlx::{PgExecutor, PgPool, types::Json};
use uuid::Uuid;

use crate::error::ApiError;

type ChallengeRow = (Uuid, Vec<u8>, Vec<u8>, DateTime<Utc>);

type DeviceRow = (Json<SignedCert>, DateTime<Utc>, Option<DateTime<Utc>>);

/// An answered challenge: who it was issued to.
pub struct IssuedChallenge {
    pub campaign: Uuid,
    pub key: PublicKey,
    pub nonce: Nonce,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct Store {
    pool: PgPool,
}

impl Store {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// The token signing key, created by whichever instance starts first.
    pub async fn token_key(&self) -> Result<DeviceKey, ApiError> {
        sqlx::query("INSERT INTO token_key (seed) VALUES ($1) ON CONFLICT DO NOTHING")
            .bind(DeviceKey::generate().seed().to_vec())
            .execute(&self.pool)
            .await?;
        let (seed,): (Vec<u8>,) = sqlx::query_as("SELECT seed FROM token_key")
            .fetch_one(&self.pool)
            .await?;
        let seed = seed
            .try_into()
            .map_err(|_| ApiError::Internal("stored token key is not 32 bytes".into()))?;
        Ok(DeviceKey::from_seed(seed))
    }

    pub async fn create_campaign(
        &self,
        name: &str,
        edition: Edition,
    ) -> Result<Campaign, ApiError> {
        let campaign = Campaign {
            id: Uuid::new_v4(),
            name: name.to_string(),
            edition,
        };
        sqlx::query(
            "INSERT INTO campaigns (id, name, edition, created_at) VALUES ($1, $2, $3, now())",
        )
        .bind(campaign.id)
        .bind(&campaign.name)
        .bind(edition.to_string())
        .execute(&self.pool)
        .await?;
        Ok(campaign)
    }

    pub async fn campaign(&self, id: Uuid) -> Result<Campaign, ApiError> {
        let row: Option<(String, String)> =
            sqlx::query_as("SELECT name, edition FROM campaigns WHERE id = $1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        let (name, edition) = row.ok_or(ApiError::NotFound)?;
        let edition: Edition =
            edition
                .parse()
                .map_err(|e: campaign_types::edition::UnknownEdition| {
                    ApiError::Internal(e.to_string())
                })?;
        Ok(Campaign { id, name, edition })
    }

    /// Deletes a campaign with its devices, setup codes, pairings and
    /// login challenges, returning what it was; `None` if there's no such
    /// campaign. Its devices can't log in from then on.
    pub async fn delete_campaign(&self, id: Uuid) -> Result<Option<(Campaign, u64)>, ApiError> {
        let campaign = match self.campaign(id).await {
            Ok(campaign) => campaign,
            Err(ApiError::NotFound) => return Ok(None),
            Err(e) => return Err(e),
        };
        let mut tx = self.pool.begin().await?;
        let (devices,): (i64,) =
            sqlx::query_as("SELECT count(*) FROM devices WHERE campaign_id = $1")
                .bind(id)
                .fetch_one(&mut *tx)
                .await?;
        sqlx::query("DELETE FROM challenges WHERE campaign_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        // Devices, setup codes and pairings go with it (ON DELETE CASCADE).
        sqlx::query("DELETE FROM campaigns WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Some((campaign, u64::try_from(devices).unwrap_or(0))))
    }

    /// A fresh one-time code that lets a device make itself DM of
    /// `campaign`.
    pub async fn new_setup_code(
        &self,
        campaign: Uuid,
        valid_for: Duration,
    ) -> Result<String, ApiError> {
        self.campaign(campaign).await?;
        let code = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 24]>());
        sqlx::query(
            "INSERT INTO setup_codes (code_hash, campaign_id, expires_at) VALUES ($1, $2, $3)",
        )
        .bind(hash(&code))
        .bind(campaign)
        .bind(Utc::now() + valid_for)
        .execute(&self.pool)
        .await?;
        Ok(code)
    }

    pub async fn peek_setup_code(&self, code: &str) -> Result<Option<Uuid>, ApiError> {
        let row: Option<(Uuid,)> = sqlx::query_as(
            "SELECT campaign_id FROM setup_codes WHERE code_hash = $1 AND expires_at > now()",
        )
        .bind(hash(code))
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(campaign,)| campaign))
    }

    /// Uses up a setup code, returning its campaign if it was valid.
    pub async fn take_setup_code(
        &self,
        executor: impl PgExecutor<'_>,
        code: &str,
    ) -> Result<Option<Uuid>, ApiError> {
        let row: Option<(Uuid,)> = sqlx::query_as(
            "DELETE FROM setup_codes WHERE code_hash = $1 AND expires_at > now() RETURNING campaign_id",
        )
        .bind(hash(code))
        .fetch_optional(executor)
        .await?;
        Ok(row.map(|(campaign,)| campaign))
    }

    pub async fn add_device(
        &self,
        executor: impl PgExecutor<'_>,
        cert: &SignedCert,
    ) -> Result<Device, ApiError> {
        let c = &cert.cert;
        let added_at = Utc::now();
        let inserted = sqlx::query(
            "INSERT INTO devices (campaign_id, public_key, role, name, cert, added_at) \
             VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING",
        )
        .bind(c.campaign)
        .bind(c.device.as_bytes().to_vec())
        .bind(role_str(c.role))
        .bind(&c.name)
        .bind(Json(cert))
        .bind(added_at)
        .execute(executor)
        .await?;
        if inserted.rows_affected() == 0 {
            return Err(ApiError::Conflict(
                "this key is already registered in the campaign".into(),
            ));
        }
        Ok(Device {
            cert: cert.clone(),
            added_at,
            revoked_at: None,
        })
    }

    pub async fn devices(&self, campaign: Uuid) -> Result<Vec<Device>, ApiError> {
        let rows: Vec<DeviceRow> = sqlx::query_as(
            "SELECT cert, added_at, revoked_at FROM devices WHERE campaign_id = $1 ORDER BY added_at",
        )
        .bind(campaign)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(Json(cert), added_at, revoked_at)| Device {
                cert,
                added_at,
                revoked_at,
            })
            .collect())
    }

    /// Revokes a device. Refuses to revoke the campaign's last DM device,
    /// which would leave nobody able to manage it.
    pub async fn revoke(&self, campaign: Uuid, key: PublicKey) -> Result<Device, ApiError> {
        let mut tx = self.pool.begin().await?;
        // Lock the campaign's devices so two DMs can't revoke each other at
        // the same moment and leave none.
        let active: Vec<(Vec<u8>, String)> = sqlx::query_as(
            "SELECT public_key, role FROM devices \
             WHERE campaign_id = $1 AND revoked_at IS NULL FOR UPDATE",
        )
        .bind(campaign)
        .fetch_all(&mut *tx)
        .await?;
        let target = active
            .iter()
            .find(|(k, _)| k.as_slice() == key.as_bytes())
            .ok_or(ApiError::NotFound)?;
        let dms = active.iter().filter(|(_, role)| role == "dm").count();
        if target.1 == "dm" && dms == 1 {
            return Err(ApiError::Unprocessable(
                "can't revoke the campaign's last DM device".into(),
            ));
        }
        let row: DeviceRow = sqlx::query_as(
            "UPDATE devices SET revoked_at = now() WHERE campaign_id = $1 AND public_key = $2 \
             RETURNING cert, added_at, revoked_at",
        )
        .bind(campaign)
        .bind(key.as_bytes().to_vec())
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        let (Json(cert), added_at, revoked_at) = row;
        Ok(Device {
            cert,
            added_at,
            revoked_at,
        })
    }

    pub async fn add_service(&self, name: &str, key: PublicKey) -> Result<(), ApiError> {
        sqlx::query(
            "INSERT INTO services (public_key, name, added_at) VALUES ($1, $2, now()) \
             ON CONFLICT (public_key) DO UPDATE SET name = EXCLUDED.name",
        )
        .bind(key.as_bytes().to_vec())
        .bind(name)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Who `key` is in `campaign`: an active device or a service.
    pub async fn principal(
        &self,
        campaign: Uuid,
        key: PublicKey,
    ) -> Result<Option<(Principal, String)>, ApiError> {
        let device: Option<(String, String)> = sqlx::query_as(
            "SELECT role, name FROM devices \
             WHERE campaign_id = $1 AND public_key = $2 AND revoked_at IS NULL",
        )
        .bind(campaign)
        .bind(key.as_bytes().to_vec())
        .fetch_optional(&self.pool)
        .await?;
        if let Some((role, name)) = device {
            let role = match role.as_str() {
                "dm" => Role::Dm,
                _ => Role::Player,
            };
            return Ok(Some((role.into(), name)));
        }
        let service: Option<(String,)> =
            sqlx::query_as("SELECT name FROM services WHERE public_key = $1")
                .bind(key.as_bytes().to_vec())
                .fetch_optional(&self.pool)
                .await?;
        if service.is_some() && self.campaign(campaign).await.is_ok() {
            return Ok(service.map(|(name,)| (Principal::Service, name)));
        }
        Ok(None)
    }

    pub async fn create_challenge(
        &self,
        campaign: Uuid,
        key: PublicKey,
        server: PublicKey,
        valid_for: Duration,
    ) -> Result<Challenge, ApiError> {
        sqlx::query("DELETE FROM challenges WHERE expires_at < now()")
            .execute(&self.pool)
            .await?;
        let challenge = Challenge {
            id: Uuid::new_v4(),
            nonce: Nonce::random(),
            campaign,
            server,
            expires_at: Utc::now() + valid_for,
        };
        sqlx::query(
            "INSERT INTO challenges (id, campaign_id, public_key, nonce, expires_at) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(challenge.id)
        .bind(campaign)
        .bind(key.as_bytes().to_vec())
        .bind(challenge.nonce.0.to_vec())
        .bind(challenge.expires_at)
        .execute(&self.pool)
        .await?;
        Ok(challenge)
    }

    /// Removes a challenge, returning it if it existed. Whoever gets it
    /// first wins, so each challenge is answered at most once.
    pub async fn take_challenge(&self, id: Uuid) -> Result<Option<IssuedChallenge>, ApiError> {
        let row: Option<ChallengeRow> = sqlx::query_as(
            "DELETE FROM challenges WHERE id = $1 \
             RETURNING campaign_id, public_key, nonce, expires_at",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        let Some((campaign, key, nonce, expires_at)) = row else {
            return Ok(None);
        };
        let corrupt = |what: &str| ApiError::Internal(format!("stored challenge has a bad {what}"));
        Ok(Some(IssuedChallenge {
            campaign,
            key: PublicKey::from_bytes(key.try_into().map_err(|_| corrupt("key"))?)
                .map_err(|_| corrupt("key"))?,
            nonce: Nonce(nonce.try_into().map_err(|_| corrupt("nonce"))?),
            expires_at,
        }))
    }
}

fn hash(code: &str) -> Vec<u8> {
    Sha256::digest(code.trim().as_bytes()).to_vec()
}

fn role_str(role: Role) -> &'static str {
    match role {
        Role::Dm => "dm",
        Role::Player => "player",
    }
}
