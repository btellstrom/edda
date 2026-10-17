use std::{
    fmt, fs,
    io::{self, Write},
    path::Path,
    str::FromStr,
};

use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::{AuthError, bytes};

/// What a signature is for. Part of every signed message, so a signature
/// can't be reused for a different purpose than the one it was made for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// A [`crate::DeviceCert`], signed by the DM device vouching for it.
    DeviceCert,
    /// A [`crate::protocol::LoginProof`], answering a login challenge.
    Login,
}

impl Purpose {
    fn label(self) -> &'static str {
        match self {
            Purpose::DeviceCert => "edda/v1/device-cert",
            Purpose::Login => "edda/v1/login",
        }
    }
}

/// The exact bytes signed for `value`: the purpose's label, a NUL (labels
/// never contain one, so the split is unambiguous), then the value's
/// canonical encoding.
fn message(purpose: Purpose, value: &impl Serialize) -> Result<Vec<u8>, AuthError> {
    let mut message = purpose.label().as_bytes().to_vec();
    message.push(0);
    postcard::to_extend(value, message).map_err(|e| AuthError::Encoding(e.to_string()))
}

/// A device's private signing key. Generated on the device and never sent
/// anywhere.
pub struct DeviceKey(SigningKey);

impl DeviceKey {
    pub fn generate() -> Self {
        Self::from_seed(rand::random())
    }

    pub fn from_seed(seed: [u8; 32]) -> Self {
        Self(SigningKey::from_bytes(&seed))
    }

    /// The 32 secret bytes everything else derives from; what to store.
    pub fn seed(&self) -> [u8; 32] {
        self.0.to_bytes()
    }

    /// Reads a key saved with [`DeviceKey::save`]; `None` if there's no
    /// such file.
    pub fn load(path: &Path) -> io::Result<Option<Self>> {
        match fs::read_to_string(path) {
            Ok(text) => {
                let seed = bytes::decode(&text)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                Ok(Some(Self::from_seed(seed)))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Writes the seed to `path`, readable by the owner only. Refuses to
    /// overwrite an existing key.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(path)?;
        writeln!(file, "{}", bytes::encode(&self.seed()))
    }

    /// Seed and public key, as PASETO v4 secret keys are laid out.
    pub fn keypair_bytes(&self) -> [u8; 64] {
        self.0.to_keypair_bytes()
    }

    pub fn public(&self) -> PublicKey {
        PublicKey(self.0.verifying_key().to_bytes())
    }

    pub fn sign(&self, purpose: Purpose, value: &impl Serialize) -> Result<Signature, AuthError> {
        Ok(Signature(self.0.sign(&message(purpose, value)?).to_bytes()))
    }
}

impl fmt::Debug for DeviceKey {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "DeviceKey({})", self.public())
    }
}

/// An Ed25519 public key. Shown as unpadded base64url.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PublicKey(#[serde(with = "bytes")] [u8; 32]);

impl PublicKey {
    /// Fails for byte strings that aren't a point on the curve.
    pub fn from_bytes(bytes: [u8; 32]) -> Result<Self, AuthError> {
        VerifyingKey::from_bytes(&bytes).map_err(|_| AuthError::InvalidKey)?;
        Ok(Self(bytes))
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// A short form for people to compare and refer to keys by.
    pub fn fingerprint(&self) -> String {
        self.to_string()[..8].to_string()
    }

    /// Checks `signature` over `value` for `purpose`. Strict verification:
    /// rejects weak keys and non-canonical signatures.
    pub fn verify(
        &self,
        purpose: Purpose,
        value: &impl Serialize,
        signature: &Signature,
    ) -> Result<(), AuthError> {
        let key = VerifyingKey::from_bytes(&self.0).map_err(|_| AuthError::InvalidKey)?;
        let signature = ed25519_dalek::Signature::from_bytes(&signature.0);
        key.verify_strict(&message(purpose, value)?, &signature)
            .map_err(|_| AuthError::BadSignature)
    }
}

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&bytes::encode(&self.0))
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "PublicKey({self})")
    }
}

impl FromStr for PublicKey {
    type Err = AuthError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_bytes(bytes::decode(s).map_err(AuthError::Encoding)?)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature(#[serde(with = "bytes")] [u8; 64]);

impl fmt::Debug for Signature {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Signature({})", bytes::encode(&self.0))
    }
}

/// 32 random bytes, for challenges.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Nonce(#[serde(with = "bytes")] pub [u8; 32]);

impl Nonce {
    pub fn random() -> Self {
        Self(rand::random())
    }
}

impl fmt::Debug for Nonce {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Nonce({})", bytes::encode(&self.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_are_bound_to_key_value_and_purpose() {
        let key = DeviceKey::generate();
        let signature = key.sign(Purpose::Login, &"hello").unwrap();
        let public = key.public();
        assert_eq!(public.verify(Purpose::Login, &"hello", &signature), Ok(()));
        assert_eq!(
            public.verify(Purpose::Login, &"hellO", &signature),
            Err(AuthError::BadSignature)
        );
        assert_eq!(
            public.verify(Purpose::DeviceCert, &"hello", &signature),
            Err(AuthError::BadSignature),
            "a login signature is no use as a certificate signature"
        );
        let other = DeviceKey::generate().public();
        assert_eq!(
            other.verify(Purpose::Login, &"hello", &signature),
            Err(AuthError::BadSignature)
        );
    }

    #[test]
    fn keys_round_trip_through_text_and_seed() {
        let key = DeviceKey::generate();
        let public = key.public();
        assert_eq!(public.to_string().len(), 43);
        assert_eq!(public.to_string().parse::<PublicKey>(), Ok(public));
        assert_eq!(DeviceKey::from_seed(key.seed()).public(), public);
        let json = serde_json::to_string(&public).unwrap();
        assert_eq!(json, format!("\"{public}\""));
        assert_eq!(serde_json::from_str::<PublicKey>(&json).unwrap(), public);
        assert!("not a key".parse::<PublicKey>().is_err());
    }
}
