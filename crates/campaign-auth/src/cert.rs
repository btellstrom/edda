use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{AuthError, DeviceKey, PublicKey, Purpose, Signature};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Runs the campaign, and can add and remove devices.
    Dm,
    Player,
}

/// A statement that `device` belongs to `campaign` with `role`, signed by
/// `issuer`: a DM device of that campaign, or for the very first DM device
/// (set up with a one-time code) the device itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceCert {
    pub campaign: Uuid,
    pub device: PublicKey,
    pub role: Role,
    /// Whose device it is, for people: "Vex's laptop".
    pub name: String,
    /// Whole seconds, so the signed encoding is a plain integer.
    #[serde(with = "chrono::serde::ts_seconds")]
    pub issued_at: DateTime<Utc>,
    pub issuer: PublicKey,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedCert {
    pub cert: DeviceCert,
    pub signature: Signature,
}

impl DeviceCert {
    /// `issuer` vouches for `device`.
    pub fn issue(
        issuer: &DeviceKey,
        campaign: Uuid,
        device: PublicKey,
        role: Role,
        name: impl Into<String>,
    ) -> Result<SignedCert, AuthError> {
        let cert = DeviceCert {
            campaign,
            device,
            role,
            name: name.into(),
            issued_at: DateTime::from_timestamp(Utc::now().timestamp(), 0).expect("now is valid"),
            issuer: issuer.public(),
        };
        let signature = issuer.sign(Purpose::DeviceCert, &cert)?;
        Ok(SignedCert { cert, signature })
    }

    /// A device vouching for itself, which also proves it holds its key.
    pub fn self_signed(
        key: &DeviceKey,
        campaign: Uuid,
        role: Role,
        name: impl Into<String>,
    ) -> Result<SignedCert, AuthError> {
        Self::issue(key, campaign, key.public(), role, name)
    }
}

impl SignedCert {
    /// Checks the signature against the certificate's own issuer. Whether
    /// that issuer may vouch for devices in the campaign is up to the
    /// caller.
    pub fn verify(&self) -> Result<&DeviceCert, AuthError> {
        self.cert
            .issuer
            .verify(Purpose::DeviceCert, &self.cert, &self.signature)?;
        Ok(&self.cert)
    }

    pub fn is_self_signed(&self) -> bool {
        self.cert.issuer == self.cert.device
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tampering_with_any_field_breaks_the_signature() {
        let dm = DeviceKey::generate();
        let player = DeviceKey::generate().public();
        let campaign = Uuid::new_v4();
        let signed =
            DeviceCert::issue(&dm, campaign, player, Role::Player, "Vex's laptop").unwrap();
        assert!(signed.verify().is_ok());
        assert!(!signed.is_self_signed());

        let tampered: [fn(&mut DeviceCert); 4] = [
            |c| c.role = Role::Dm,
            |c| c.campaign = Uuid::new_v4(),
            |c| c.name.push('!'),
            |c| c.device = DeviceKey::generate().public(),
        ];
        for tamper in tampered {
            let mut forged = signed.clone();
            tamper(&mut forged.cert);
            assert_eq!(forged.verify(), Err(AuthError::BadSignature));
        }
        // Claiming another issuer doesn't help either.
        let mut forged = signed.clone();
        forged.cert.issuer = DeviceKey::generate().public();
        assert_eq!(forged.verify(), Err(AuthError::BadSignature));
    }

    #[test]
    fn certificates_survive_json() {
        let key = DeviceKey::generate();
        let signed = DeviceCert::self_signed(&key, Uuid::new_v4(), Role::Dm, "DM laptop").unwrap();
        assert!(signed.is_self_signed());
        let json = serde_json::to_string(&signed).unwrap();
        let back: SignedCert = serde_json::from_str(&json).unwrap();
        assert_eq!(back, signed);
        assert!(back.verify().is_ok());
    }
}
