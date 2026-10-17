//! Pairing devices with a short code: the DM's device ("host") and a new
//! device ("joiner") run the PAKE in `edda_auth::pairing`, relayed by
//! the auth service.
//!
//! ```text
//! host                         relay                         joiner
//!  | POST /pairings  ─────────▶  slot 7
//!  | shows 7-tavern-dragon                   (told the code out loud)
//!  |                                ◀────────── joiner/0: SPAKE2 message
//!  | host/0: SPAKE2 message ──────▶
//!  |                                ◀────────── joiner/1: sealed {key, name}
//!  | opens it (or cancels: wrong code), asks the DM, adds the device
//!  | host/1: sealed {campaign, server key, certificate} ─▶
//! ```

use std::time::Duration;

use chrono::{DateTime, Utc};
use edda_auth::{
    DeviceCert, DeviceKey, PublicKey, Role, SignedCert,
    pairing::{Exchange, JoinAnswer, JoinRequest, PairingCode, PairingError, Side},
    protocol::{Campaign, Device, PairingMessage, PairingSlot},
};
use reqwest::StatusCode;

use crate::{ClientError, auth::AuthClient, check, send};

#[derive(Debug, thiserror::Error)]
pub enum PairError {
    #[error(transparent)]
    Client(#[from] ClientError),
    #[error(transparent)]
    Pairing(#[from] PairingError),
    #[error("nobody joined before the code expired")]
    Expired,
    #[error("this code has already been used, has expired, or was cancelled")]
    Unavailable,
    /// The joiner's message didn't open: they used a different code (or
    /// someone guessed). The pairing has been cancelled.
    #[error("the code didn't match; the pairing was cancelled")]
    WrongCode,
    #[error("the DM declined")]
    Declined,
    #[error("the DM's answer doesn't check out: {0}")]
    BadAnswer(&'static str),
}

/// A code the host can hand out.
#[derive(Debug, Clone)]
pub struct Invitation {
    pub code: PairingCode,
    pub role: Role,
    pub expires_at: DateTime<Utc>,
}

/// What a joiner gets out of a successful pairing.
#[derive(Debug, Clone)]
pub struct Joined {
    pub campaign: Campaign,
    /// The auth service's key, vouched for by the DM's device.
    pub server_key: PublicKey,
    pub cert: SignedCert,
}

impl AuthClient {
    /// Opens a pairing (DM devices only) and makes up its code. The words
    /// are made here and never sent anywhere.
    pub async fn invite(&self, role: Role) -> Result<Invitation, ClientError> {
        let slot: PairingSlot = send(self.http.post(self.url("/pairings"))).await?;
        Ok(Invitation {
            code: PairingCode::generate(slot.slot),
            role,
            expires_at: slot.expires_at,
        })
    }

    pub async fn cancel_pairing(&self, slot: u16) -> Result<(), ClientError> {
        let url = self.url(&format!("/pairings/{slot}"));
        check(self.http.delete(url).send().await?).await.map(drop)
    }

    async fn relay_send(
        &self,
        slot: u16,
        side: Side,
        n: u8,
        data: Vec<u8>,
    ) -> Result<(), PairError> {
        let url = self.url(&format!("/pairings/{slot}/{}/{n}", side.as_str()));
        let response = self
            .http
            .put(url)
            .json(&PairingMessage { data })
            .send()
            .await;
        match check(response.map_err(ClientError::from)?).await {
            Ok(_) => Ok(()),
            Err(ClientError::NotFound | ClientError::Exhausted(_)) => Err(PairError::Unavailable),
            Err(e) => Err(e.into()),
        }
    }

    /// Waits for a message from `side`, until `deadline`.
    async fn relay_receive(
        &self,
        slot: u16,
        side: Side,
        n: u8,
        deadline: DateTime<Utc>,
    ) -> Result<Vec<u8>, PairError> {
        let url = self.url(&format!("/pairings/{slot}/{}/{n}", side.as_str()));
        loop {
            let response = check(
                self.http
                    .get(&url)
                    .send()
                    .await
                    .map_err(ClientError::from)?,
            )
            .await;
            match response {
                Ok(r) if r.status() == StatusCode::NO_CONTENT => {
                    if Utc::now() > deadline {
                        return Err(PairError::Expired);
                    }
                }
                Ok(r) => {
                    let message: PairingMessage =
                        r.json().await.map_err(ClientError::InvalidResponse)?;
                    return Ok(message.data);
                }
                Err(ClientError::NotFound) => return Err(PairError::Unavailable),
                Err(e) => {
                    // A blip shouldn't end the pairing; keep trying until
                    // the deadline.
                    if Utc::now() > deadline {
                        return Err(e.into());
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }
}

/// The host's side: waits for someone to join with `invitation`'s code,
/// asks `approve` about them, and if approved admits them to the campaign.
/// `dm` must be logged in as the DM device `key`.
pub async fn host(
    dm: &AuthClient,
    key: &DeviceKey,
    campaign: &Campaign,
    server_key: PublicKey,
    invitation: &Invitation,
    approve: impl FnOnce(&JoinRequest) -> bool,
) -> Result<Device, PairError> {
    let slot = invitation.code.slot;
    let result = run_host(dm, key, campaign, server_key, invitation, approve).await;
    if matches!(result, Err(PairError::WrongCode | PairError::Pairing(_))) {
        // Burn the code: one guess is all anyone gets.
        let _ = dm.cancel_pairing(slot).await;
    }
    result
}

async fn run_host(
    dm: &AuthClient,
    key: &DeviceKey,
    campaign: &Campaign,
    server_key: PublicKey,
    invitation: &Invitation,
    approve: impl FnOnce(&JoinRequest) -> bool,
) -> Result<Device, PairError> {
    let slot = invitation.code.slot;
    let deadline = invitation.expires_at;
    let (exchange, ours) = Exchange::start(Side::Host, &invitation.code);
    let theirs = dm.relay_receive(slot, Side::Joiner, 0, deadline).await?;
    dm.relay_send(slot, Side::Host, 0, ours).await?;
    let mut channel = exchange.finish(&theirs)?;

    let sealed = dm.relay_receive(slot, Side::Joiner, 1, deadline).await?;
    let request: JoinRequest = channel.open(&sealed).map_err(|e| match e {
        PairingError::WrongCode => PairError::WrongCode,
        e => e.into(),
    })?;

    if !approve(&request) {
        dm.relay_send(slot, Side::Host, 1, channel.seal(&JoinAnswer::Declined))
            .await?;
        return Err(PairError::Declined);
    }
    let cert = DeviceCert::issue(
        key,
        campaign.id,
        request.key,
        invitation.role,
        &request.name,
    )
    .map_err(|e| ClientError::Rejected(e.to_string()))?;
    let device = dm.add_device(&cert).await?;
    let answer = JoinAnswer::Approved {
        campaign: campaign.clone(),
        server_key,
        cert: Box::new(cert),
    };
    dm.relay_send(slot, Side::Host, 1, channel.seal(&answer))
        .await?;
    Ok(device)
}

/// The joiner's side: pairs `key` using a code from the DM. On success the
/// device is in the campaign and can log in.
pub async fn join(
    auth: &AuthClient,
    code: &PairingCode,
    key: &DeviceKey,
    name: &str,
) -> Result<Joined, PairError> {
    let slot = code.slot;
    // The host has up to the pairing's lifetime to answer; the relay tells
    // us sooner if the pairing is cancelled.
    let deadline = Utc::now() + chrono::Duration::minutes(10);
    let (exchange, ours) = Exchange::start(Side::Joiner, code);
    auth.relay_send(slot, Side::Joiner, 0, ours).await?;
    let theirs = auth.relay_receive(slot, Side::Host, 0, deadline).await?;
    let mut channel = exchange.finish(&theirs)?;

    let request = JoinRequest {
        key: key.public(),
        name: name.to_string(),
    };
    auth.relay_send(slot, Side::Joiner, 1, channel.seal(&request))
        .await?;
    let sealed = match auth.relay_receive(slot, Side::Host, 1, deadline).await {
        // The host cancels when our message doesn't open for it.
        Err(PairError::Unavailable) => return Err(PairError::WrongCode),
        other => other?,
    };
    let (campaign, server_key, cert) = match channel.open(&sealed)? {
        JoinAnswer::Approved {
            campaign,
            server_key,
            cert,
        } => (campaign, server_key, *cert),
        JoinAnswer::Declined => return Err(PairError::Declined),
    };

    // Trust the DM, but check what can be checked.
    let issued = cert
        .verify()
        .map_err(|_| PairError::BadAnswer("certificate signature is invalid"))?;
    if issued.device != key.public() || issued.campaign != campaign.id || issued.name != name {
        return Err(PairError::BadAnswer("certificate is for something else"));
    }
    // The DM's device knows the auth service's real key; make sure this is
    // the service we're actually talking to.
    if auth.server_key().await? != server_key {
        return Err(PairError::BadAnswer(
            "this auth server isn't the one your DM uses",
        ));
    }
    Ok(Joined {
        campaign,
        server_key,
        cert,
    })
}
