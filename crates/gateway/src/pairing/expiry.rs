//! Time formatting and pair-request expiry predicates.

use {moltis_db::Dialect, time::OffsetDateTime};

pub(super) fn current_epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub(super) fn format_datetime(dt: OffsetDateTime) -> String {
    dt.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

/// SQL predicate for still-pending pair requests; `expired_expiry_sql` is the
/// inverse. `expires_at` is a TEXT timestamp (RFC3339 or `YYYY-MM-DD HH:MM:SS`,
/// always UTC): SQLite parses both with julianday, PostgreSQL compares wall
/// times so naive and `Z`-suffixed values stay comparable. Values that parse as
/// neither make the predicate NULL — the row stays pending, never expires,
/// matching julianday's NULL semantics.
pub(super) fn pending_expiry_sql(dialect: Dialect) -> &'static str {
    match dialect {
        Dialect::Sqlite => "julianday(expires_at) > julianday(?)",
        Dialect::Postgres => {
            "CASE WHEN pg_input_is_valid(expires_at, 'timestamp') \
             THEN expires_at::timestamp END > ?::timestamp"
        },
    }
}

/// Inverse of [`pending_expiry_sql`] for eviction.
pub(super) fn expired_expiry_sql(dialect: Dialect) -> &'static str {
    match dialect {
        Dialect::Sqlite => "julianday(expires_at) <= julianday(?)",
        Dialect::Postgres => {
            "CASE WHEN pg_input_is_valid(expires_at, 'timestamp') \
             THEN expires_at::timestamp END <= ?::timestamp"
        },
    }
}

pub(super) fn is_expired(expires_at: &str) -> bool {
    let Ok(expires) =
        OffsetDateTime::parse(expires_at, &time::format_description::well_known::Rfc3339)
    else {
        // If we can't parse, try the database datetime format.
        return is_expired_db_timestamp(expires_at);
    };
    OffsetDateTime::now_utc() > expires
}

fn is_expired_db_timestamp(expires_at: &str) -> bool {
    // Database datetime format: "YYYY-MM-DD HH:MM:SS"
    // Simple string comparison works because the format is lexicographically ordered.
    let now = OffsetDateTime::now_utc();
    let now_str = format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        now.year(),
        now.month() as u8,
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    );
    now_str.as_str() > expires_at
}
