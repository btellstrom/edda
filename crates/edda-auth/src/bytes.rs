//! Serde for fixed-size byte arrays: unpadded base64url in human-readable
//! formats (JSON), raw bytes otherwise (the canonical postcard encoding
//! that gets signed).

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserializer, Serializer, de};

pub fn encode(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn decode<const N: usize>(text: &str) -> Result<[u8; N], String> {
    let bytes = URL_SAFE_NO_PAD
        .decode(text.trim())
        .map_err(|e| e.to_string())?;
    let len = bytes.len();
    bytes
        .try_into()
        .map_err(|_| format!("expected {N} bytes, got {len}"))
}

pub fn serialize<S: Serializer, const N: usize>(bytes: &[u8; N], s: S) -> Result<S::Ok, S::Error> {
    if s.is_human_readable() {
        s.serialize_str(&encode(bytes))
    } else {
        s.serialize_bytes(bytes)
    }
}

pub fn deserialize<'de, D: Deserializer<'de>, const N: usize>(d: D) -> Result<[u8; N], D::Error> {
    struct Visitor<const N: usize>;

    impl<'de, const N: usize> de::Visitor<'de> for Visitor<N> {
        type Value = [u8; N];

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "{N} bytes, base64url-encoded")
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            decode(v).map_err(E::custom)
        }

        fn visit_bytes<E: de::Error>(self, v: &[u8]) -> Result<Self::Value, E> {
            v.try_into().map_err(|_| E::invalid_length(v.len(), &self))
        }
    }

    if d.is_human_readable() {
        d.deserialize_str(Visitor)
    } else {
        d.deserialize_bytes(Visitor)
    }
}

/// The same for variable-length byte strings: `#[serde(with = "bytes::vec")]`.
pub mod vec {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use serde::{Deserialize, Deserializer, Serializer, de};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        if s.is_human_readable() {
            s.serialize_str(&URL_SAFE_NO_PAD.encode(bytes))
        } else {
            s.serialize_bytes(bytes)
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        if d.is_human_readable() {
            let text = String::deserialize(d)?;
            URL_SAFE_NO_PAD
                .decode(text.trim())
                .map_err(de::Error::custom)
        } else {
            serde_bytes_vec(d)
        }
    }

    fn serde_bytes_vec<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        struct Visitor;
        impl de::Visitor<'_> for Visitor {
            type Value = Vec<u8>;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("bytes")
            }
            fn visit_bytes<E: de::Error>(self, v: &[u8]) -> Result<Self::Value, E> {
                Ok(v.to_vec())
            }
        }
        d.deserialize_bytes(Visitor)
    }
}
