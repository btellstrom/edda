//! For services: issuing and checking tokens, and an axum extractor.

use std::{sync::Arc, time::Duration};

use axum::{
    Json,
    extract::FromRequestParts,
    http::{StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
};
use pasetors::{
    Public,
    claims::{Claims as PasetoClaims, ClaimsValidationRules},
    keys::{AsymmetricPublicKey, AsymmetricSecretKey},
    public,
    token::UntrustedToken,
    version4::V4,
};

use crate::{AuthError, DeviceKey, PublicKey, protocol::Claims};

/// Bound into every token's signature, so these tokens can't be confused
/// with PASETO tokens minted by anything else using the same key.
const IMPLICIT_ASSERTION: &[u8] = b"edda/v1/token";
const CLAIMS_KEY: &str = "edda";

/// Mints tokens. Only the auth service holds one.
pub struct TokenSigner {
    secret: AsymmetricSecretKey<V4>,
    public: PublicKey,
}

impl TokenSigner {
    /// Signs tokens with `key`.
    pub fn new(key: &DeviceKey) -> Self {
        Self {
            secret: AsymmetricSecretKey::from(&key.keypair_bytes()).expect("64-byte keypair"),
            public: key.public(),
        }
    }

    /// The key its tokens can be checked with.
    pub fn public_key(&self) -> PublicKey {
        self.public
    }

    /// A token for `claims`, valid until `claims.expires_at`.
    pub fn issue(&self, claims: &Claims) -> Result<String, AuthError> {
        let lifetime = (claims.expires_at - chrono::Utc::now())
            .to_std()
            .unwrap_or(Duration::ZERO);
        let encoding = |e: pasetors::errors::Error| AuthError::Encoding(e.to_string());
        let mut paseto = PasetoClaims::new_expires_in(&lifetime).map_err(encoding)?;
        let ours = serde_json::to_value(claims).map_err(|e| AuthError::Encoding(e.to_string()))?;
        paseto.add_additional(CLAIMS_KEY, ours).map_err(encoding)?;
        public::sign(&self.secret, &paseto, None, Some(IMPLICIT_ASSERTION)).map_err(encoding)
    }
}

/// Checks tokens. Needs only the auth service's public key.
pub struct TokenVerifier {
    public: AsymmetricPublicKey<V4>,
}

impl TokenVerifier {
    /// Accepts tokens signed with `key`.
    pub fn new(key: PublicKey) -> Self {
        Self {
            public: AsymmetricPublicKey::from(key.as_bytes()).expect("32-byte key"),
        }
    }

    /// The claims of a genuine, unexpired token.
    pub fn verify(&self, token: &str) -> Result<Claims, AuthError> {
        let untrusted =
            UntrustedToken::<Public, V4>::try_from(token).map_err(|_| AuthError::InvalidToken)?;
        // Checks the signature, then `exp`, `nbf` and `iat`.
        let trusted = public::verify(
            &self.public,
            &untrusted,
            &ClaimsValidationRules::new(),
            None,
            Some(IMPLICIT_ASSERTION),
        )
        .map_err(|_| AuthError::InvalidToken)?;
        let ours = trusted
            .payload_claims()
            .and_then(|c| c.get_claim(CLAIMS_KEY))
            .ok_or(AuthError::InvalidToken)?;
        serde_json::from_value(ours.clone()).map_err(|_| AuthError::InvalidToken)
    }
}

/// The verified claims of the request's bearer token. Requires the
/// [`TokenVerifier`] as an `Extension<Arc<TokenVerifier>>` layer.
pub struct Authenticated(pub Claims);

impl<S: Send + Sync> FromRequestParts<S> for Authenticated {
    type Rejection = Unauthorized;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        let verifier = parts
            .extensions
            .get::<Arc<TokenVerifier>>()
            .expect("TokenVerifier extension is installed");
        let token = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or(Unauthorized("a bearer token is required"))?;
        verifier
            .verify(token.trim())
            .map(Authenticated)
            .map_err(|_| Unauthorized("invalid or expired token"))
    }
}

/// A `401`, saying why.
#[derive(Debug)]
pub struct Unauthorized(pub &'static str);

impl IntoResponse for Unauthorized {
    fn into_response(self) -> Response {
        (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
            Json(serde_json::json!({ "error": self.0 })),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use uuid::Uuid;

    use super::*;
    use crate::protocol::Principal;

    fn claims(lifetime: chrono::Duration) -> Claims {
        Claims {
            key: DeviceKey::generate().public(),
            campaign: Uuid::new_v4(),
            role: Principal::Player,
            name: "Vex's laptop".into(),
            edition: Default::default(),
            expires_at: Utc::now() + lifetime,
        }
    }

    #[test]
    fn tokens_verify_only_with_the_right_key_and_until_they_expire() {
        let signer = TokenSigner::new(&DeviceKey::generate());
        let verifier = TokenVerifier::new(signer.public_key());
        let claims = claims(chrono::Duration::minutes(15));
        let token = signer.issue(&claims).unwrap();
        assert!(token.starts_with("v4.public."));
        assert_eq!(verifier.verify(&token), Ok(claims.clone()));

        let stranger = TokenVerifier::new(DeviceKey::generate().public());
        assert_eq!(stranger.verify(&token), Err(AuthError::InvalidToken));

        let mut tampered = token.clone();
        let last = tampered.pop().unwrap();
        tampered.push(if last == 'A' { 'B' } else { 'A' });
        assert_eq!(verifier.verify(&tampered), Err(AuthError::InvalidToken));

        let expired = signer
            .issue(&self::claims(chrono::Duration::seconds(-5)))
            .unwrap();
        assert_eq!(verifier.verify(&expired), Err(AuthError::InvalidToken));
    }
}
