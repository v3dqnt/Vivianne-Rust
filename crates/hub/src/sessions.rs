//! Bearer tokens. A token is random bytes from the operating system, shown once; only its SHA-256 is stored, so a
//! copied database signs nobody in. Tokens expire and can be revoked. There are no passwords.

use std::fmt::Write as _;

use rand::TryRngCore as _;
use rand::rngs::OsRng;
use rusqlite::{OptionalExtension as _, params};
use sha2::{Digest as _, Sha256};
use vivianne_projects::model::UserId;
use vivianne_projects::store::{StoreError, Stored};

use crate::sqlite::{Database, unavailable};

const PREFIX: &str = "vivs_";
pub const MAX_USER: usize = 64;

/// `bytes` random bytes as hex, for tokens and ids. Fails rather than hand out anything guessable.
pub fn random_hex(bytes: usize) -> Stored<String> {
    let mut buffer = vec![0_u8; bytes];
    OsRng.try_fill_bytes(&mut buffer).map_err(unavailable)?;
    Ok(buffer
        .iter()
        .fold(String::with_capacity(bytes * 2), |mut hex, b| {
            let _ = write!(hex, "{b:02x}");
            hex
        }))
}

fn hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// A user id is what other people type to invite someone, so it is kept to plain characters.
#[must_use]
pub fn valid_user(user: &str) -> bool {
    (1..=MAX_USER).contains(&user.len())
        && user
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn millis(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

impl Database {
    /// A new token for `user`, good until `now + lifetime_ms`.
    pub fn issue_token(&self, user: &UserId, now: u64, lifetime_ms: u64) -> Stored<String> {
        if !valid_user(&user.0) {
            return Err(StoreError::Unavailable);
        }
        let token = format!("{PREFIX}{}", random_hex(32)?);
        self.connection()
            .execute(
                "INSERT INTO sessions (hash, user, expires_at) VALUES (?1, ?2, ?3)",
                params![
                    hash(&token),
                    user.0,
                    millis(now.saturating_add(lifetime_ms))
                ],
            )
            .map_err(unavailable)?;
        Ok(token)
    }

    /// Whose token this is, if it is known and not expired.
    pub fn user_of(&self, token: &str, now: u64) -> Stored<Option<UserId>> {
        self.connection()
            .query_row(
                "SELECT user FROM sessions WHERE hash = ?1 AND expires_at > ?2",
                params![hash(token), millis(now)],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map(|found| found.map(UserId))
            .map_err(unavailable)
    }

    /// Every token of `user` stops working. Returns how many there were.
    pub fn revoke_user(&self, user: &UserId) -> Stored<usize> {
        self.connection()
            .execute("DELETE FROM sessions WHERE user = ?1", [&user.0])
            .map_err(unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_works_until_it_expires_or_is_revoked_and_is_not_stored() {
        let db = Database::in_memory().expect("opens");
        let ann = UserId::from("ann");
        let token = db.issue_token(&ann, 100, 50).expect("issued");
        assert_eq!(db.user_of(&token, 149), Ok(Some(ann.clone())));
        assert_eq!(db.user_of(&token, 150), Ok(None), "expired");
        assert_eq!(db.user_of("vivs_guess", 0), Ok(None));
        let stored: Vec<u8> = db
            .connection()
            .query_row("SELECT hash FROM sessions", [], |r| r.get(0))
            .expect("row");
        assert_ne!(stored, token.as_bytes());
        assert_eq!(db.revoke_user(&ann), Ok(1));
        assert_eq!(db.user_of(&token, 101), Ok(None));
    }

    #[test]
    fn a_user_id_with_odd_characters_gets_no_token() {
        let db = Database::in_memory().expect("opens");
        for user in ["", "a b", "a/b", "../x", &"a".repeat(65)] {
            assert!(db.issue_token(&user.into(), 0, 1).is_err(), "{user}");
        }
    }
}
