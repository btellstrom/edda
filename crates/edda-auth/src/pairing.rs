//! Pairing a new device with a short code, using a PAKE (SPAKE2).
//!
//! The DM's device shows a code like `7-tavern-dragon`. The number picks the
//! pairing on the auth service and is public; the two words are a password
//! that never leaves the two devices. Both run SPAKE2 with the words, with
//! the auth service relaying the messages, and end up with the same strong
//! key only if they used the same words. An eavesdropper (the relay
//! included) learns nothing it could guess the words against offline; an
//! active attacker gets one online guess, after which the pairing is gone.
//!
//! Over the resulting encrypted channel the new device sends its public key
//! and name, and the DM's device answers with a certificate for it, the
//! campaign, and the auth service's key, which the new device then trusts
//! because it came from the DM, not from whatever server it happens to be
//! talking to.
//!
//! Everything here is IO-free; `edda-client` moves the bytes.

use std::{fmt, str::FromStr, sync::LazyLock};

use chacha20poly1305::{ChaCha20Poly1305, KeyInit, aead::Aead};
use hkdf::Hkdf;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::Sha256;
use spake2::{Ed25519Group, Identity, Password, Spake2};

use crate::{PublicKey, SignedCert, protocol::Campaign};

/// Words in a code. With 1,296 words, two give a 1 in 1,679,616 chance to
/// an attacker, who only gets one try per code.
pub const WORDS: usize = 2;

static WORDLIST: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    include_str!("eff_short_wordlist_1.txt")
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .collect()
});

/// What can go wrong reading a pairing code or a sealed message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PairingError {
    #[error("a pairing code looks like 7-tavern-dragon")]
    Malformed,
    #[error("{0:?} is not one of the code words; check the spelling")]
    UnknownWord(String),
    /// Decryption failed: the other side used different words (or the
    /// messages were tampered with).
    #[error("the codes don't match")]
    WrongCode,
    #[error("the other side sent something unexpected")]
    BadMessage,
}

/// `<slot>-<word>-<word>`.
#[derive(Clone, PartialEq, Eq)]
pub struct PairingCode {
    pub slot: u16,
    words: [String; WORDS],
}

impl PairingCode {
    /// A code for `slot` with fresh random words.
    pub fn generate(slot: u16) -> Self {
        let list = &*WORDLIST;
        let words = std::array::from_fn(|_| list[rand::random_range(0..list.len())].to_string());
        Self { slot, words }
    }

    fn password(&self) -> Password {
        Password::new(self.words.join("-"))
    }
}

impl fmt::Display for PairingCode {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}-{}", self.slot, self.words.join("-"))
    }
}

impl fmt::Debug for PairingCode {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "PairingCode({}-…)", self.slot)
    }
}

impl FromStr for PairingCode {
    type Err = PairingError;

    /// Forgiving about case, spaces and separators: "7 Tavern dragon" works.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<String> = s
            .split(|c: char| c == '-' || c.is_whitespace())
            .filter(|p| !p.is_empty())
            .map(str::to_lowercase)
            .collect();
        let [slot, words @ ..] = parts.as_slice() else {
            return Err(PairingError::Malformed);
        };
        let slot = slot.parse().map_err(|_| PairingError::Malformed)?;
        let words: [String; WORDS] = words
            .to_vec()
            .try_into()
            .map_err(|_| PairingError::Malformed)?;
        if let Some(unknown) = words.iter().find(|w| !WORDLIST.contains(&w.as_str())) {
            return Err(PairingError::UnknownWord(unknown.clone()));
        }
        Ok(Self { slot, words })
    }
}

/// The two sides of a pairing. SPAKE2 is asymmetric, and each side's
/// identity is bound into the key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// The DM's device, which made the code.
    Host,
    /// The device joining.
    Joiner,
}

impl Side {
    /// The side's name, `host` or `joiner`: in the relay's paths, and bound
    /// into the keys.
    pub fn as_str(self) -> &'static str {
        match self {
            Side::Host => "host",
            Side::Joiner => "joiner",
        }
    }

    fn other(self) -> Side {
        match self {
            Side::Host => Side::Joiner,
            Side::Joiner => Side::Host,
        }
    }
}

/// One side's SPAKE2 run, between sending its message and receiving the
/// other's.
pub struct Exchange {
    side: Side,
    slot: u16,
    spake: Spake2<Ed25519Group>,
}

impl Exchange {
    /// Starts the exchange; send the returned message to the other side.
    pub fn start(side: Side, code: &PairingCode) -> (Self, Vec<u8>) {
        // Binding the slot means a code's words are only good for its slot.
        let host = Identity::new(format!("edda/v1/pair/host/{}", code.slot).as_bytes());
        let joiner = Identity::new(format!("edda/v1/pair/joiner/{}", code.slot).as_bytes());
        let (spake, message) = match side {
            Side::Host => Spake2::start_a(&code.password(), &host, &joiner),
            Side::Joiner => Spake2::start_b(&code.password(), &host, &joiner),
        };
        let exchange = Self {
            side,
            slot: code.slot,
            spake,
        };
        (exchange, message)
    }

    /// Finishes with the other side's message. Always succeeds for a
    /// well-formed message, even if the words differ; that only shows when
    /// the first encrypted message fails to open.
    pub fn finish(self, theirs: &[u8]) -> Result<Channel, PairingError> {
        let shared = self
            .spake
            .finish(theirs)
            .map_err(|_| PairingError::BadMessage)?;
        let hkdf = Hkdf::<Sha256>::new(Some(&self.slot.to_be_bytes()), &shared);
        let key = |side: Side| {
            let mut key = [0u8; 32];
            let info = format!("edda/v1/pair/{}-to-other", side.as_str());
            hkdf.expand(info.as_bytes(), &mut key)
                .expect("32 bytes is a valid length");
            ChaCha20Poly1305::new(&key.into())
        };
        Ok(Channel {
            send: key(self.side),
            receive: key(self.side.other()),
            sent: 0,
            received: 0,
        })
    }
}

/// Authenticated encryption both ways, with a key per direction. Nonces are
/// message counters, so no key ever sees a nonce twice.
pub struct Channel {
    send: ChaCha20Poly1305,
    receive: ChaCha20Poly1305,
    sent: u64,
    received: u64,
}

impl Channel {
    /// Encrypts `message` for the other side.
    pub fn seal(&mut self, message: &impl Serialize) -> Vec<u8> {
        let plain = serde_json::to_vec(message).expect("pairing messages serialize");
        let sealed = self
            .send
            .encrypt(&nonce(self.sent), plain.as_slice())
            .expect("encryption can't fail for in-memory buffers");
        self.sent += 1;
        sealed
    }

    /// Decrypts a message from the other side; fails if it was sealed with
    /// another code.
    pub fn open<T: DeserializeOwned>(&mut self, sealed: &[u8]) -> Result<T, PairingError> {
        let plain = self
            .receive
            .decrypt(&nonce(self.received), sealed)
            .map_err(|_| PairingError::WrongCode)?;
        self.received += 1;
        serde_json::from_slice(&plain).map_err(|_| PairingError::BadMessage)
    }
}

fn nonce(counter: u64) -> chacha20poly1305::Nonce {
    let mut nonce = [0u8; 12];
    nonce[4..].copy_from_slice(&counter.to_be_bytes());
    nonce.into()
}

/// The joiner's first encrypted message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct JoinRequest {
    pub key: PublicKey,
    /// "Vex's laptop".
    pub name: String,
}

/// The DM's answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JoinAnswer {
    Approved {
        campaign: Campaign,
        /// The auth service's key, as the DM's device knows it.
        server_key: PublicKey,
        cert: Box<SignedCert>,
    },
    Declined,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(host_code: &PairingCode, joiner_code: &PairingCode) -> (Channel, Channel) {
        let (host, to_joiner) = Exchange::start(Side::Host, host_code);
        let (joiner, to_host) = Exchange::start(Side::Joiner, joiner_code);
        (
            host.finish(&to_host).unwrap(),
            joiner.finish(&to_joiner).unwrap(),
        )
    }

    #[test]
    fn the_same_code_gives_both_sides_a_working_channel() {
        let code = PairingCode::generate(7);
        let (mut host, mut joiner) = pair(&code, &code);
        let request = JoinRequest {
            key: crate::DeviceKey::generate().public(),
            name: "Vex's laptop".into(),
        };
        let sealed = joiner.seal(&request);
        assert_eq!(host.open::<JoinRequest>(&sealed), Ok(request));
        let sealed = host.seal(&JoinAnswer::Declined);
        assert_eq!(joiner.open::<JoinAnswer>(&sealed), Ok(JoinAnswer::Declined));
    }

    #[test]
    fn a_different_word_or_slot_gives_nothing() {
        let code: PairingCode = "7-acid-acorn".parse().unwrap();
        for wrong in ["7-acid-acre", "8-acid-acorn"] {
            let (mut host, mut joiner) = pair(&code, &wrong.parse().unwrap());
            let sealed = joiner.seal(&"hello");
            assert_eq!(
                host.open::<String>(&sealed),
                Err(PairingError::WrongCode),
                "{wrong}"
            );
        }
    }

    #[test]
    fn messages_cannot_be_tampered_with_reordered_or_reflected() {
        let code = PairingCode::generate(3);
        let (mut host, mut joiner) = pair(&code, &code);
        let mut sealed = joiner.seal(&"first");
        sealed[0] ^= 1;
        assert_eq!(host.open::<String>(&sealed), Err(PairingError::WrongCode));

        let (mut host, mut joiner) = pair(&code, &code);
        let first = joiner.seal(&"first");
        let second = joiner.seal(&"second");
        assert_eq!(
            host.open::<String>(&second),
            Err(PairingError::WrongCode),
            "out of order"
        );
        assert_eq!(host.open::<String>(&first), Ok("first".into()));

        // A message can't be bounced back to its sender as if from the
        // other side: each direction has its own key.
        let (_, mut joiner) = pair(&code, &code);
        let own = joiner.seal(&"echo");
        assert_eq!(joiner.open::<String>(&own), Err(PairingError::WrongCode));
    }

    #[test]
    fn codes_parse_forgivingly_and_reject_typos() {
        let code = PairingCode::generate(42);
        let text = code.to_string();
        assert_eq!(text.parse::<PairingCode>(), Ok(code.clone()));
        assert_eq!(
            text.to_uppercase().replace('-', " ").parse::<PairingCode>(),
            Ok(code)
        );
        assert_eq!(
            "7-tavrn-acid".parse::<PairingCode>(),
            Err(PairingError::UnknownWord("tavrn".into()))
        );
        for malformed in [
            "",
            "acid-acorn",
            "7-acid",
            "7-acid-acorn-acre",
            "x-acid-acorn",
        ] {
            assert_eq!(
                malformed.parse::<PairingCode>(),
                Err(PairingError::Malformed)
            );
        }
        assert_eq!(WORDLIST.len(), 1296);
        assert!(
            format!("{:?}", PairingCode::generate(1)).ends_with("1-…)"),
            "Debug hides the words"
        );
    }
}
