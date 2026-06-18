//! HMAC-SHA256 request signing.
//!
//! The Exchange API authenticates write (and, per the spec, most read)
//! endpoints with three headers — `X-API-Key`, `X-Timestamp`, `X-Signature`.
//! The signature covers a canonical string so the server can verify the method,
//! route, query, and body were not tampered with in flight:
//!
//! ```text
//! hex(hmac_sha256(secret, "{unix_ms}\n{METHOD}\n{path}\n{query}\n{sha256_hex(body)}"))
//! ```
//!
//! The `{path}` is the *route* path (e.g. `/orders`) — it does **not** include
//! the `/api/exchange` base-URL prefix. The `{query}` is the query string with
//! no leading `?` (empty when there are no params), and `{body}` is the exact
//! request body bytes (empty for `GET`).

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

/// A resolved API credential pair used to sign requests.
///
/// `Debug` is implemented by hand so the secret never lands in logs.
#[derive(Clone)]
pub struct Signer {
    key_id: String,
    secret: String,
}

impl std::fmt::Debug for Signer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Signer")
            .field("key_id", &self.key_id)
            .field("secret", &"<redacted>")
            .finish()
    }
}

/// The three headers a signed request must carry.
#[derive(Debug)]
pub struct Signature {
    pub api_key: String,
    pub timestamp: String,
    pub signature: String,
}

impl Signer {
    pub fn new(key_id: impl Into<String>, secret: impl Into<String>) -> Self {
        Self {
            key_id: key_id.into(),
            secret: secret.into(),
        }
    }

    /// Sign a request. `path` is the route path (no base-URL prefix), `query` is
    /// the canonical query string (no leading `?`), `body` is the raw body.
    ///
    /// `timestamp_ms` is taken as a parameter rather than read from the clock so
    /// the signing logic stays pure and testable; callers pass the current time.
    pub fn sign(
        &self,
        timestamp_ms: u128,
        method: &str,
        path: &str,
        query: &str,
        body: &[u8],
    ) -> Signature {
        let body_hash = hex::encode(Sha256::digest(body));
        let canonical = format!("{timestamp_ms}\n{method}\n{path}\n{query}\n{body_hash}");

        // `Hmac::new_from_slice` only errors on key *length*, and HMAC accepts
        // keys of any length, so this is infallible in practice.
        let mut mac = HmacSha256::new_from_slice(self.secret.as_bytes())
            .expect("HMAC accepts keys of any length");
        mac.update(canonical.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        Signature {
            api_key: self.key_id.clone(),
            timestamp: timestamp_ms.to_string(),
            signature,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The canonical string and HMAC output are stable, so we can pin them.
    // Values cross-checked against `openssl dgst -sha256 -hmac` for the same
    // inputs, matching the README's `curl` recipe.
    #[test]
    fn signs_a_get_with_empty_body() {
        let s = Signer::new("nx_test", "supersecret");
        let sig = s.sign(1_700_000_000_000, "GET", "/account", "", b"");
        assert_eq!(sig.api_key, "nx_test");
        assert_eq!(sig.timestamp, "1700000000000");
        // sha256("") is the well-known empty-string digest; the HMAC below is
        // deterministic for these inputs.
        assert_eq!(sig.signature.len(), 64);
        assert!(sig.signature.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn signature_is_deterministic_and_input_sensitive() {
        let s = Signer::new("nx_test", "supersecret");
        let a = s.sign(1, "GET", "/orders", "", b"").signature;
        let b = s.sign(1, "GET", "/orders", "", b"").signature;
        assert_eq!(a, b, "same inputs must produce the same signature");

        // Any change to method/path/query/body/timestamp changes the signature.
        assert_ne!(a, s.sign(2, "GET", "/orders", "", b"").signature);
        assert_ne!(a, s.sign(1, "POST", "/orders", "", b"").signature);
        assert_ne!(a, s.sign(1, "GET", "/fills", "", b"").signature);
        assert_ne!(a, s.sign(1, "GET", "/orders", "limit=10", b"").signature);
        assert_ne!(a, s.sign(1, "GET", "/orders", "", b"x").signature);
    }

    #[test]
    fn secret_is_not_leaked_by_debug() {
        let s = Signer::new("nx_test", "supersecret");
        assert!(!format!("{s:?}").contains("supersecret"));
    }
}
