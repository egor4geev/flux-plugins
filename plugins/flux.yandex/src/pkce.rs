//! PKCE for Yandex ID's sign-in (RFC 7636, S256): a random verifier, its challenge — the SHA-256 of
//! the verifier in base64url — and a random `state` that ties the browser's return to this sign-in.
//! With the verifier the code is exchanged without a client secret.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

/// One sign-in's secrets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pkce {
    /// Sent with the code to `/token`.
    pub verifier: String,
    /// Sent to `/authorize`.
    pub challenge: String,
    /// Sent to `/authorize`, comes back with the code.
    pub state: String,
}

impl Pkce {
    /// Fresh random ones; none when the system's randomness isn't there.
    pub fn new() -> Option<Pkce> {
        let verifier = random(32)?;
        let state = random(16)?;
        Some(Pkce {
            challenge: challenge(&verifier),
            verifier,
            state,
        })
    }
}

/// `bytes` random bytes as base64url: 32 bytes — a verifier of 43 characters.
fn random(bytes: usize) -> Option<String> {
    let mut buffer = vec![0u8; bytes];
    getrandom::fill(&mut buffer).ok()?;
    Some(URL_SAFE_NO_PAD.encode(buffer))
}

/// The S256 challenge of a verifier.
pub fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc_7636_vector() {
        // RFC 7636, appendix B.
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn fresh_ones_differ() {
        let one = Pkce::new().unwrap();
        let two = Pkce::new().unwrap();
        assert_eq!(one.verifier.len(), 43);
        assert_ne!(one.verifier, two.verifier);
        assert_ne!(one.state, two.state);
        assert_eq!(one.challenge, challenge(&one.verifier));
    }
}
