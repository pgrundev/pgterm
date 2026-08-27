//! Every string that can reach the screen, a log file, or a test transcript
//! passes through here first. pgbot already redacts DSNs in its own errors
//! (internal/conn/redact.go); this is the belt to that suspenders — plus a
//! literal scrub of the resolved secret itself, which only this process knows.

use std::fmt;

/// Redacts credentials from arbitrary text:
/// 1. every literal occurrence of `secret` (the resolved env value),
/// 2. URL userinfo passwords — `scheme://user:pass@` → `scheme://user:REDACTED@`,
/// 3. keyword DSNs — `password=x`, `password='x y'`, `PASSWORD = "x"` → `password=REDACTED`.
pub fn redact(text: &str, secret: Option<&str>) -> String {
    let mut out = match secret {
        Some(s) if !s.is_empty() => text.replace(s, "REDACTED"),
        _ => text.to_string(),
    };
    out = redact_url_passwords(&out);
    redact_keyword_passwords(&out)
}

fn redact_url_passwords(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find("://") {
        let after = pos + 3;
        out.push_str(&rest[..after]);
        rest = &rest[after..];
        // Userinfo runs to the LAST '@' before the authority ends ('/'), since
        // passwords may themselves contain '@'. No '@' → no userinfo.
        let authority_end = rest
            .find(|c: char| c == '/' || c.is_whitespace())
            .unwrap_or(rest.len());
        let authority = &rest[..authority_end];
        if let Some(at) = authority.rfind('@') {
            let userinfo = &authority[..at];
            if let Some(colon) = userinfo.find(':') {
                out.push_str(&userinfo[..colon]);
                out.push_str(":REDACTED");
            } else {
                out.push_str(userinfo);
            }
            out.push_str(&authority[at..]);
        } else {
            out.push_str(authority);
        }
        rest = &rest[authority_end..];
    }
    out.push_str(rest);
    out
}

fn redact_keyword_passwords(text: &str) -> String {
    let lower = text.to_lowercase();
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while let Some(rel) = lower[i..].find("password") {
        let start = i + rel;
        let mut j = start + "password".len();
        out.push_str(&text[i..j]);
        // optional spaces, '=', optional spaces
        let mut k = j;
        while k < bytes.len() && bytes[k] == b' ' {
            k += 1;
        }
        if k >= bytes.len() || bytes[k] != b'=' {
            i = j;
            continue;
        }
        k += 1;
        while k < bytes.len() && bytes[k] == b' ' {
            k += 1;
        }
        out.push_str(&text[j..k]); // the "= " connective, spacing preserved
        j = k;
        // value: quoted or bare (to next whitespace)
        let end = if j < bytes.len() && (bytes[j] == b'\'' || bytes[j] == b'"') {
            let quote = bytes[j];
            match text[j + 1..].find(quote as char) {
                Some(q) => j + 1 + q + 1,
                None => text.len(),
            }
        } else {
            j + text[j..]
                .find(char::is_whitespace)
                .unwrap_or(text.len() - j)
        };
        out.push_str("REDACTED");
        i = end;
    }
    out.push_str(&text[i..]);
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// The profile's environment variable is unset or empty.
    EnvMissing,
    /// pgbot exited 3 — connection or execution failure.
    ConnectionFailed,
    /// The pgbot child outlived its deadline and was killed.
    Timeout,
    /// PGBOT_BIN (or pgbot on PATH) could not be spawned.
    PgbotMissing,
    /// pgbot succeeded but its stdout was not the JSON we expected.
    BadOutput,
    /// pgbot exited 64 — our invocation was malformed (a bug here, not there).
    Usage,
}

/// An error safe to render: the message was redacted at construction, so no
/// later formatting step can leak what the constructor already scrubbed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafeError {
    pub kind: ErrorKind,
    pub message: String,
}

impl SafeError {
    pub fn new(kind: ErrorKind, raw: &str, secret: Option<&str>) -> Self {
        SafeError {
            kind,
            message: redact(raw.trim(), secret),
        }
    }
}

impl fmt::Display for SafeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self.kind {
            ErrorKind::EnvMissing => "environment variable missing",
            ErrorKind::ConnectionFailed => "connection failed",
            ErrorKind::Timeout => "timed out",
            ErrorKind::PgbotMissing => "pgbot not found",
            ErrorKind::BadOutput => "unexpected pgbot output",
            ErrorKind::Usage => "internal error (bad pgbot invocation)",
        };
        if self.message.is_empty() {
            write!(f, "{label}")
        } else {
            write!(f, "{label}: {}", self.message)
        }
    }
}

impl std::error::Error for SafeError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_password_is_redacted() {
        let got = redact(
            "could not connect to postgres://alex:super-secret@host:5432/db",
            None,
        );
        assert_eq!(
            got,
            "could not connect to postgres://alex:REDACTED@host:5432/db"
        );
    }

    #[test]
    fn literal_secret_is_redacted_wherever_it_appears() {
        let secret = "postgres://alex:super-secret@host/db";
        let raw = format!("dial error for {secret} (tried twice)");
        let got = redact(&raw, Some(secret));
        assert!(!got.contains("super-secret"), "{got}");
        assert!(!got.contains("postgres://alex"), "{got}");
    }

    #[test]
    fn url_rule_alone_covers_an_unknown_secret() {
        // Even when the caller has no secret in hand, a DSN in child stderr is caught.
        let got = redact(
            "pgbot: parse \"postgresql://u:pw@h/db?sslmode=require\": bad",
            None,
        );
        assert!(!got.contains(":pw@"), "{got}");
        assert!(got.contains("u:REDACTED@h"), "{got}");
    }

    #[test]
    fn password_with_at_sign_is_fully_redacted() {
        let got = redact("postgres://u:p@ss@host/db", None);
        assert_eq!(got, "postgres://u:REDACTED@host/db");
    }

    #[test]
    fn url_without_password_is_untouched() {
        let s = "postgres://readonly@host/db and http://example.com/path";
        assert_eq!(redact(s, None), s);
    }

    #[test]
    fn keyword_dsn_variants() {
        assert_eq!(
            redact("host=h user=u password=hunter2 dbname=d", None),
            "host=h user=u password=REDACTED dbname=d"
        );
        assert_eq!(
            redact("password='p w' host=h", None),
            "password=REDACTED host=h"
        );
        assert_eq!(
            redact("PASSWORD = \"x y\" rest", None),
            "PASSWORD = REDACTED rest"
        );
        assert_eq!(redact("password=x", None), "password=REDACTED");
    }

    #[test]
    fn secret_with_regex_metachars_is_plain_text() {
        let secret = "p.*[a](b)^$?";
        let raw = format!("bad url p.*[a](b)^$? here and password={secret}");
        let got = redact(&raw, Some(secret));
        assert!(!got.contains("p.*[a]"), "{got}");
    }

    #[test]
    fn empty_and_plain_text_pass_through() {
        assert_eq!(redact("", None), "");
        assert_eq!(
            redact("3 blocked queries on public.orders", None),
            "3 blocked queries on public.orders"
        );
    }

    #[test]
    fn safe_error_display_is_one_line_and_redacted() {
        let e = SafeError::new(
            ErrorKind::ConnectionFailed,
            "pgbot: connect postgres://a:pw@h/db: connection refused\n",
            Some("postgres://a:pw@h/db"),
        );
        let s = e.to_string();
        assert!(s.starts_with("connection failed: "), "{s}");
        assert!(!s.contains(":pw@"), "{s}");
        assert!(!s.contains('\n'), "{s}");
    }

    #[test]
    fn safe_error_kinds_have_distinct_labels() {
        let kinds = [
            ErrorKind::EnvMissing,
            ErrorKind::ConnectionFailed,
            ErrorKind::Timeout,
            ErrorKind::PgbotMissing,
            ErrorKind::BadOutput,
            ErrorKind::Usage,
        ];
        let labels: Vec<String> = kinds
            .iter()
            .map(|k| {
                SafeError {
                    kind: *k,
                    message: String::new(),
                }
                .to_string()
            })
            .collect();
        let mut dedup = labels.clone();
        dedup.dedup();
        assert_eq!(labels.len(), dedup.len(), "{labels:?}");
    }
}
