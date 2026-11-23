//! This machine's stable multiplayer identity: a secret key whose public half is the pairing code the user shares
//! with a partner. Made once and kept, so a partner's saved code keeps working. The file is the old app's
//! (`~/.viv/agent/multiplayer_key`, 64 hex digits), so an existing identity carries over.
//!
//! It authenticates a machine, not a person, and its public half is meant to be said out loud, so it is a plain
//! file next to the rest of `~/.viv/agent` rather than something in the keychain.

use std::path::Path;

use iroh::SecretKey;

fn key_file() -> std::path::PathBuf {
    vivianne_ai::paths::viv_home_path(&["agent", "multiplayer_key"])
}

fn to_hex(bytes: &[u8; 32]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::with_capacity(64), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

fn from_hex(text: &str) -> Option<[u8; 32]> {
    let text = text.trim();
    if text.len() != 64 || !text.is_ascii() {
        return None;
    }
    let mut out = [0_u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// The key stored at `path`, or a new one written there.
///
/// # Errors
/// The key could not be kept: using it would give a different code every launch, so this is an error and not a
/// shrug.
pub fn load_or_create_at(path: &Path) -> Result<SecretKey, String> {
    if let Some(bytes) = std::fs::read_to_string(path)
        .ok()
        .as_deref()
        .and_then(from_hex)
    {
        return Ok(SecretKey::from_bytes(&bytes));
    }
    let key = SecretKey::generate();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("couldn't create {}: {e}", dir.display()))?;
    }
    std::fs::write(path, to_hex(&key.to_bytes()))
        .map_err(|e| format!("couldn't save the pairing identity: {e}"))?;
    Ok(key)
}

/// This machine's key.
///
/// # Errors
/// See [`load_or_create_at`].
pub fn load_or_create() -> Result<SecretKey, String> {
    load_or_create_at(&key_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_is_made_once_and_is_the_same_the_next_time() {
        let path = std::env::temp_dir().join(format!("viv-mp-key-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let first = load_or_create_at(&path).expect("makes one");
        let again = load_or_create_at(&path).expect("reads it");
        assert_eq!(first.public().to_string(), again.public().to_string());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn hex_round_trips_and_rejects_the_wrong_length() {
        let bytes = [7_u8; 32];
        assert_eq!(from_hex(&to_hex(&bytes)), Some(bytes));
        assert_eq!(from_hex("abc"), None);
        assert_eq!(from_hex(&"é".repeat(32)), None);
    }
}
