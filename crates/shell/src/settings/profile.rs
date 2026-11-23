//! The person's profile: a name, a photo and a banner, kept in `~/.viv/profile/`. It is only ever shown in this app
//! (the Account page); nothing here is sent anywhere. A picked image is copied into the folder under a new name each
//! time, so a changed picture is never confused with the one drawn before, and the old copy is removed.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// The largest picture taken.
pub const MAX_IMAGE_BYTES: u64 = 12 * 1024 * 1024;
const EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "webp", "gif"];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Saved {
    #[serde(default)]
    name: String,
    /// File names inside the profile folder.
    #[serde(default)]
    avatar: Option<String>,
    #[serde(default)]
    banner: Option<String>,
}

/// What the Account page shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub avatar: Option<PathBuf>,
    pub banner: Option<PathBuf>,
}

/// Which picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Avatar,
    Banner,
}

fn dir() -> PathBuf {
    vivianne_ai::paths::viv_home_path(&["profile"])
}

fn file(dir: &Path) -> PathBuf {
    dir.join("profile.json")
}

static CACHE: Mutex<Option<Profile>> = Mutex::new(None);

/// A name for someone who has not chosen one: the account's own, else "You".
fn default_name() -> String {
    std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .ok()
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "You".to_owned())
}

fn read_in(dir: &Path) -> Profile {
    let saved: Saved = std::fs::read_to_string(file(dir))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let existing = |name: Option<String>| name.map(|n| dir.join(n)).filter(|p| p.is_file());
    Profile {
        name: if saved.name.trim().is_empty() {
            default_name()
        } else {
            saved.name
        },
        avatar: existing(saved.avatar),
        banner: existing(saved.banner),
    }
}

/// The profile, read from disk the first time and kept after.
#[must_use]
pub fn current() -> Profile {
    let mut cache = CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cache.get_or_insert_with(|| read_in(&dir())).clone()
}

fn write_in(dir: &Path, profile: &Profile) -> Result<(), String> {
    let name_of = |p: &Option<PathBuf>| {
        p.as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
    };
    let saved = Saved {
        name: profile.name.clone(),
        avatar: name_of(&profile.avatar),
        banner: name_of(&profile.banner),
    };
    vivianne_ai::paths::write_json_atomic(&file(dir), &saved)
}

fn update(change: impl FnOnce(&mut Profile) -> Result<(), String>) -> Result<(), String> {
    let mut cache = CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut profile = cache.take().unwrap_or_else(|| read_in(&dir()));
    let result = change(&mut profile).and_then(|()| write_in(&dir(), &profile));
    *cache = Some(profile);
    result
}

/// Sets the display name. An empty name goes back to the default.
pub fn set_name(name: &str) -> Result<(), String> {
    let name: String = name.trim().chars().take(60).collect();
    update(|p| {
        p.name = if name.is_empty() {
            default_name()
        } else {
            name
        };
        Ok(())
    })
}

/// Why a file cannot be a profile picture, if it cannot.
pub fn check_image(source: &Path) -> Result<String, String> {
    let ext = source
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if !EXTENSIONS.contains(&ext.as_str()) {
        return Err(format!("A picture is a {} file.", EXTENSIONS.join(", ")));
    }
    let size = std::fs::metadata(source).map_err(|e| e.to_string())?.len();
    if size > MAX_IMAGE_BYTES {
        return Err(format!(
            "That picture is over {} MB.",
            MAX_IMAGE_BYTES / 1024 / 1024
        ));
    }
    Ok(ext)
}

/// Copies `source` in as the avatar or banner and removes the one it replaces.
pub fn set_image(kind: Kind, source: &Path) -> Result<(), String> {
    let ext = check_image(source)?;
    let folder = dir();
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let label = if kind == Kind::Avatar {
        "avatar"
    } else {
        "banner"
    };
    let name = format!("{label}-{stamp}.{ext}");
    std::fs::copy(source, folder.join(&name)).map_err(|e| e.to_string())?;
    update(|p| {
        let slot = if kind == Kind::Avatar {
            &mut p.avatar
        } else {
            &mut p.banner
        };
        if let Some(old) = slot.replace(folder.join(&name)) {
            // The old copy is ours to remove; if it is already gone, nothing is lost.
            let _ = std::fs::remove_file(old);
        }
        Ok(())
    })
}

/// Goes back to no picture.
pub fn clear_image(kind: Kind) -> Result<(), String> {
    update(|p| {
        let slot = if kind == Kind::Avatar {
            &mut p.avatar
        } else {
            &mut p.banner
        };
        if let Some(old) = slot.take() {
            let _ = std::fs::remove_file(old);
        }
        Ok(())
    })
}

/// Up to two capital letters for a name with no photo.
#[must_use]
pub fn initials(name: &str) -> String {
    let letters: String = name
        .split_whitespace()
        .filter_map(|w| w.chars().next())
        .take(2)
        .flat_map(char::to_uppercase)
        .collect();
    if letters.is_empty() {
        "?".to_owned()
    } else {
        letters
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("viv-profile-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("creates");
        dir
    }

    #[test]
    fn initials_are_the_first_letters_of_up_to_two_words() {
        assert_eq!(initials("ada lovelace byron"), "AL");
        assert_eq!(initials("vedant"), "V");
        assert_eq!(initials("   "), "?");
    }

    #[test]
    fn only_small_pictures_of_known_kinds_are_taken() {
        let dir = temp();
        let png = dir.join("a.PNG");
        std::fs::write(&png, b"x").expect("writes");
        assert_eq!(check_image(&png).expect("ok"), "png");
        let exe = dir.join("a.exe");
        std::fs::write(&exe, b"x").expect("writes");
        assert!(check_image(&exe).is_err());
        let big = dir.join("big.jpg");
        let file = std::fs::File::create(&big).expect("creates");
        file.set_len(MAX_IMAGE_BYTES + 1).expect("sizes");
        assert!(check_image(&big).unwrap_err().contains("MB"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_saved_profile_reads_back_and_a_missing_picture_is_dropped() {
        let dir = temp();
        let avatar = dir.join("avatar-1.png");
        std::fs::write(&avatar, b"x").expect("writes");
        write_in(
            &dir,
            &Profile {
                name: "Ada".into(),
                avatar: Some(avatar.clone()),
                banner: Some(dir.join("banner-gone.png")),
            },
        )
        .expect("saves");
        let back = read_in(&dir);
        assert_eq!((back.name.as_str(), back.avatar), ("Ada", Some(avatar)));
        assert!(back.banner.is_none(), "a picture that is gone is not shown");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
