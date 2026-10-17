//! Identity and authentication for Edda.
//!
//! Every device (a player's laptop, the DM's phone) holds an Ed25519
//! [`DeviceKey`] that never leaves it. A campaign's DM devices vouch for
//! other devices by signing a [`DeviceCert`]; the auth service only accepts
//! devices with a valid certificate. To use the services, a device answers a
//! login challenge by signing it, and gets a short-lived token (PASETO
//! v4.public) that every service can check with the auth service's public
//! key alone.
//!
//! Anything signed is signed as `"edda/v1/<purpose>\0"` followed
//! by the value's canonical (postcard) encoding, so a signature made for one
//! purpose can never be passed off as another.

mod bytes;
mod cert;
mod keys;
pub mod pairing;
pub mod protocol;
#[cfg(feature = "server")]
pub mod server;

pub use cert::{DeviceCert, Role, SignedCert};
pub use keys::{DeviceKey, Nonce, PublicKey, Purpose, Signature};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AuthError {
    #[error("not a valid public key")]
    InvalidKey,
    #[error("signature does not match")]
    BadSignature,
    #[error("invalid encoding: {0}")]
    Encoding(String),
    #[error("invalid or expired token")]
    InvalidToken,
}
