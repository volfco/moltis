use std::borrow::Cow;

/// SQL dialect of the connected backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    Sqlite,
    Postgres,
}

impl Dialect {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Sqlite => "sqlite",
            Self::Postgres => "postgres",
        }
    }

    #[must_use]
    pub fn is_postgres(self) -> bool {
        matches!(self, Self::Postgres)
    }

    /// SQL expression producing the current UTC time as `YYYY-MM-DD HH:MM:SS`.
    ///
    /// Timestamps are stored as text so both backends sort identically and no
    /// driver-side date decoding is involved.
    #[must_use]
    pub fn now_expr(self) -> &'static str {
        match self {
            Self::Sqlite => "datetime('now')",
            Self::Postgres => "to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS')",
        }
    }

    /// Default value clause for a `TEXT` timestamp column.
    #[must_use]
    pub fn now_default(self) -> &'static str {
        match self {
            Self::Sqlite => "(datetime('now'))",
            Self::Postgres => "(to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS'))",
        }
    }
}

/// Rewrite `?` placeholders into PostgreSQL's numbered `$1`, `$2`, … form.
///
/// SQLite queries are written with `?`; PostgreSQL rejects that syntax. Every
/// placeholder consumes the next index in textual order, which matches the
/// order binds are applied, so positional binding stays identical.
///
/// Placeholders inside string literals, quoted identifiers and comments are
/// left alone — only real parameter markers are rewritten. A `?` that is part
/// of a `?|` / `?&` (JSON) or `??` operator sequence is also left alone.
#[must_use]
pub fn rewrite_placeholders(dialect: Dialect, sql: &str) -> Cow<'_, str> {
    if !dialect.is_postgres() || !sql.contains('?') {
        return Cow::Borrowed(sql);
    }

    let mut out = String::with_capacity(sql.len() + 8);
    let mut index = 0usize;
    let mut chars = sql.char_indices().peekable();

    while let Some((pos, c)) = chars.next() {
        match c {
            '\'' => {
                out.push(c);
                // Copy a single-quoted literal verbatim, honouring '' escapes.
                while let Some((_, inner)) = chars.next() {
                    out.push(inner);
                    if inner == '\'' {
                        if matches!(chars.peek(), Some((_, '\''))) {
                            chars.next();
                            out.push('\'');
                        } else {
                            break;
                        }
                    }
                }
            },
            '"' => {
                out.push(c);
                for (_, inner) in chars.by_ref() {
                    out.push(inner);
                    if inner == '"' {
                        break;
                    }
                }
            },
            '-' if sql[pos..].starts_with("--") => {
                out.push_str("--");
                chars.next();
                for (_, inner) in chars.by_ref() {
                    out.push(inner);
                    if inner == '\n' {
                        break;
                    }
                }
            },
            '/' if sql[pos..].starts_with("/*") => {
                out.push_str("/*");
                chars.next();
                while let Some((_, inner)) = chars.next() {
                    out.push(inner);
                    if inner == '*' && matches!(chars.peek(), Some((_, '/'))) {
                        chars.next();
                        out.push('/');
                        break;
                    }
                }
            },
            '?' => match chars.peek().copied() {
                // `??` is sqlx's escape for a literal `?`; `?|`, `?&` are JSON ops.
                Some((_, '?')) => {
                    out.push('?');
                    chars.next();
                    out.push('?');
                },
                Some((_, '|')) | Some((_, '&')) => out.push('?'),
                _ => {
                    index += 1;
                    out.push('$');
                    out.push_str(&index.to_string());
                },
            },
            _ => out.push(c),
        }
    }

    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn pg(sql: &str) -> String {
        rewrite_placeholders(Dialect::Postgres, sql).into_owned()
    }

    #[test]
    fn sqlite_sql_is_untouched() {
        let sql = "SELECT a FROM t WHERE b = ?";
        assert_eq!(rewrite_placeholders(Dialect::Sqlite, sql), sql);
    }

    #[test]
    fn numbers_placeholders_in_order() {
        assert_eq!(
            pg("SELECT a FROM t WHERE b = ? AND c = ? LIMIT ?"),
            "SELECT a FROM t WHERE b = $1 AND c = $2 LIMIT $3"
        );
    }

    #[test]
    fn reused_placeholder_gets_distinct_index() {
        assert_eq!(
            pg("UPDATE t SET a = ? WHERE b = ?"),
            "UPDATE t SET a = $1 WHERE b = $2"
        );
    }

    #[test]
    fn literals_are_not_rewritten() {
        assert_eq!(
            pg("SELECT '?, ?' AS x, a FROM t WHERE b = ?"),
            "SELECT '?, ?' AS x, a FROM t WHERE b = $1"
        );
    }

    #[test]
    fn escaped_quote_literal_is_preserved() {
        assert_eq!(
            pg("SELECT 'it''s ?' FROM t WHERE b = ?"),
            "SELECT 'it''s ?' FROM t WHERE b = $1"
        );
    }

    #[test]
    fn quoted_identifier_is_not_rewritten() {
        assert_eq!(
            pg("SELECT \"we?ird\" FROM t WHERE b = ?"),
            "SELECT \"we?ird\" FROM t WHERE b = $1"
        );
    }

    #[test]
    fn comments_are_not_rewritten() {
        assert_eq!(
            pg("SELECT a FROM t -- what? \n WHERE b = ? /* why? */"),
            "SELECT a FROM t -- what? \n WHERE b = $1 /* why? */"
        );
    }

    #[test]
    fn json_operators_are_left_alone() {
        assert_eq!(
            pg("SELECT a ?| ARRAY['k'] FROM t"),
            "SELECT a ?| ARRAY['k'] FROM t"
        );
        // A real placeholder before a JSON operator still gets numbered.
        assert_eq!(
            pg("SELECT a ?| ARRAY[?] FROM t WHERE b = ?"),
            "SELECT a ?| ARRAY[$1] FROM t WHERE b = $2"
        );
    }

    #[test]
    fn dollar_placeholders_are_not_double_rewritten() {
        assert_eq!(
            pg("SELECT a FROM t WHERE b = $1"),
            "SELECT a FROM t WHERE b = $1"
        );
    }
}
