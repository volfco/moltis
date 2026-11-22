use {
    argon2::{
        Argon2,
        password_hash::{
            PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng,
        },
    },
    moltis_db::Dialect,
    sha2::{Digest, Sha256},
};

/// Pre-computed Argon2 hash used for constant-time dummy verification when no password is set.
pub(crate) const DUMMY_ARGON2_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

pub fn is_loopback(ip: &str) -> bool {
    ip == "127.0.0.1" || ip.starts_with("127.") || ip == "::1" || ip.starts_with("::ffff:127.")
}

pub(crate) fn hash_password(password: &str) -> crate::Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| crate::Error::Crypto(format!("failed to hash password: {e}")))?;
    Ok(hash.to_string())
}

pub(crate) fn verify_password(password: &str, hash_str: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash_str) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

pub(crate) fn generate_token() -> String {
    use {base64::Engine, rand::Rng};

    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub(crate) fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub(crate) fn hmac_sha256_hex(input: &str, salt: &str) -> String {
    use hmac::{Hmac, Mac};

    type HmacSha256 = Hmac<Sha256>;
    let Ok(mut mac) = HmacSha256::new_from_slice(salt.as_bytes()) else {
        return sha256_hex(input);
    };
    mac.update(input.as_bytes());
    format!("{:x}", mac.finalize().into_bytes())
}

pub(crate) fn safe_equal(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let diff = a
        .as_bytes()
        .iter()
        .zip(b.as_bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y));
    diff == 0
}

/// Format a stored `YYYY-MM-DD HH:MM:SS` UTC timestamp as ISO-8601
/// `YYYY-MM-DDTHH:MM:SSZ`. Both backends store timestamps as that text form,
/// so the conversion happens here instead of in dialect-specific SQL.
pub(crate) fn iso_timestamp(value: &str) -> String {
    match value.split_once(' ') {
        Some((date, time)) => format!("{date}T{time}Z"),
        None => value.to_owned(),
    }
}

/// SQL expression for the current UTC time shifted by `days` days, rendered
/// as `YYYY-MM-DD HH:MM:SS` text for comparison against stored timestamps.
pub(crate) fn days_from_now_sql(dialect: Dialect, days: i64) -> String {
    match dialect {
        Dialect::Sqlite => format!("datetime('now', '{days:+} days')"),
        Dialect::Postgres => {
            format!(
                "to_char(now() AT TIME ZONE 'utc' + interval '{days} days', 'YYYY-MM-DD HH24:MI:SS')"
            )
        },
    }
}
