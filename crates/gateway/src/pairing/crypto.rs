//! Token generation and Ed25519 challenge-response helpers.

use sha2::{Digest, Sha256};

pub(super) fn generate_token() -> String {
    use {base64::Engine, rand::Rng};

    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub(super) fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Verify an Ed25519 challenge-response: check that `signature` is a valid
/// signature over `nonce` by the holder of `public_key_b64`.
///
/// All three values are base64-encoded. Returns `Ok(true)` on valid signature,
/// `Ok(false)` on invalid, `Err` on malformed inputs.
pub fn verify_ed25519_challenge(
    public_key_b64: &str,
    nonce: &[u8],
    signature_b64: &str,
) -> Result<bool, String> {
    use {base64::Engine, ed25519_dalek::Verifier};

    let pk_bytes = base64::engine::general_purpose::STANDARD
        .decode(public_key_b64)
        .map_err(|e| format!("invalid public key base64: {e}"))?;

    let pk_array: [u8; 32] = pk_bytes
        .try_into()
        .map_err(|_| "public key must be exactly 32 bytes".to_string())?;

    let verifying_key = ed25519_dalek::VerifyingKey::from_bytes(&pk_array)
        .map_err(|e| format!("invalid Ed25519 public key: {e}"))?;

    let sig_bytes = base64::engine::general_purpose::STANDARD
        .decode(signature_b64)
        .map_err(|e| format!("invalid signature base64: {e}"))?;

    let sig_array: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| "signature must be exactly 64 bytes".to_string())?;

    let signature = ed25519_dalek::Signature::from_bytes(&sig_array);

    Ok(verifying_key.verify(nonce, &signature).is_ok())
}

/// Generate a 32-byte cryptographic challenge nonce, returned as base64.
pub fn generate_challenge_nonce() -> (Vec<u8>, String) {
    use {base64::Engine, rand::Rng};

    let mut nonce = [0u8; 32];
    rand::rng().fill_bytes(&mut nonce);
    let b64 = base64::engine::general_purpose::STANDARD.encode(nonce);
    (nonce.to_vec(), b64)
}

/// Compute a fingerprint from a base64-encoded Ed25519 public key:
/// `SHA256:<base64 of SHA-256 hash>`.
pub fn public_key_fingerprint(public_key_b64: &str) -> Result<String, String> {
    use base64::Engine;

    let pk_bytes = base64::engine::general_purpose::STANDARD
        .decode(public_key_b64)
        .map_err(|e| format!("invalid public key base64: {e}"))?;
    let pk_array: [u8; 32] = pk_bytes
        .as_slice()
        .try_into()
        .map_err(|_| "public key must be exactly 32 bytes".to_string())?;
    ed25519_dalek::VerifyingKey::from_bytes(&pk_array)
        .map_err(|e| format!("invalid Ed25519 public key: {e}"))?;
    let hash = Sha256::digest(&pk_bytes);
    Ok(format!(
        "SHA256:{}",
        base64::engine::general_purpose::STANDARD.encode(hash)
    ))
}
