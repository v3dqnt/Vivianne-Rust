//! The saved-thread format as a chain of versions. Port of the idea behind the original `session-format` and
//! `session-format-catalog`: a file says which format it is in, an older one is carried forward one adjacent step at a
//! time, a newer one is refused (never read as if it were current, and never written over), and a file that cannot be
//! read is told apart from one that merely needs upgrading. The whole chain is checked for gaps before anything is read.
//!
//! The original's migrations (v0 to v4) turn its event-log format into later ones. Vivianne has taken two steps: version 0
//! (no `version` field) to version 1 (`version: 1`, still one JSON object per thread), and version 1 to version 2, where
//! the thread became an append-only log (`log.rs`). The step to 2 changes nothing in the object itself; the new container
//! is written by the store when it carries an old file forward.

use serde_json::Value;

/// The format this build writes.
pub const CURRENT: u32 = 2;

/// The oldest format this build can still read.
pub const OLDEST: u32 = 0;

/// One adjacent step: `from` becomes `from + 1`.
struct Step {
    from: u32,
    apply: fn(Value) -> Result<Value, String>,
}

/// Every step, oldest first. Adding a format means appending one step here and raising [`CURRENT`].
const STEPS: &[Step] = &[
    Step {
        from: 0,
        apply: v0_to_v1,
    },
    Step {
        from: 1,
        apply: v1_to_v2,
    },
];

/// What a file says about itself, read from the object without restoring anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Classification {
    /// Written in the current format.
    Current,
    /// Older, and carried forward by [`restore`].
    MigrationRequired { from: u32 },
    /// Newer than this build understands, or older than the oldest it still reads.
    Unsupported { version: u32 },
    /// Not a thread at all.
    Malformed { reason: String },
}

/// Why a file was not restored. Each keeps the source untouched: nothing here rewrites or deletes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatError {
    Malformed(String),
    /// Written by a newer build (or one too old to read): opening it here and saving would lose what it holds.
    Unsupported {
        version: u32,
    },
    /// A step failed; the file is as it was.
    Migration {
        from: u32,
        why: String,
    },
}

impl std::fmt::Display for FormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed(reason) => write!(f, "not a saved conversation: {reason}"),
            Self::Unsupported { version } if *version > CURRENT => write!(
                f,
                "written by a newer Vivianne (format {version}; this one reads up to {CURRENT}); it was left as it is"
            ),
            Self::Unsupported { version } => write!(
                f,
                "format {version} is older than this Vivianne can read (oldest is {OLDEST})"
            ),
            Self::Migration { from, why } => {
                write!(f, "could not bring format {from} forward: {why}")
            }
        }
    }
}

impl std::error::Error for FormatError {}

/// The format number of a parsed file: its `version`, or 0 when it has none (every file written before versions existed).
fn version_of(value: &Value) -> Result<u32, String> {
    match value.get("version") {
        None => Ok(0),
        Some(v) => v
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| "`version` is not a whole number".to_owned()),
    }
}

/// Reads only the header facts: is it a thread, and which format. No history is looked at beyond its being a list.
#[must_use]
pub fn classify(value: &Value) -> Classification {
    let Some(object) = value.as_object() else {
        return Classification::Malformed {
            reason: "the file is not a JSON object".to_owned(),
        };
    };
    for (field, is_ok) in [
        ("id", object.get("id").is_some_and(Value::is_string)),
        (
            "workspace",
            object.get("workspace").is_some_and(Value::is_string),
        ),
        (
            "history",
            object.get("history").is_some_and(Value::is_array),
        ),
    ] {
        if !is_ok {
            return Classification::Malformed {
                reason: format!("`{field}` is missing or of the wrong kind"),
            };
        }
    }
    match version_of(value) {
        Err(reason) => Classification::Malformed { reason },
        Ok(version) if version == CURRENT => Classification::Current,
        #[allow(clippy::absurd_extreme_comparisons, clippy::manual_range_contains)]
        Ok(version) if version > CURRENT || version < OLDEST => {
            Classification::Unsupported { version }
        }
        Ok(from) => Classification::MigrationRequired { from },
    }
}

/// Checks that the steps form one unbroken chain from [`OLDEST`] to [`CURRENT`]. A broken chain is a bug of this build,
/// found before any file is read.
pub fn check_chain() -> Result<(), String> {
    let mut at = OLDEST;
    for step in STEPS {
        if step.from != at {
            return Err(format!("the format chain jumps from {at} to {}", step.from));
        }
        at += 1;
    }
    if at == CURRENT {
        Ok(())
    } else {
        Err(format!(
            "the format chain ends at {at}, but the current format is {CURRENT}"
        ))
    }
}

/// A thread's JSON, in the current format, and where it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct Restored {
    pub value: Value,
    /// The format it was in, when that was not the current one.
    pub migrated_from: Option<u32>,
}

/// Brings parsed file contents to the current format, one adjacent step at a time.
pub fn restore(mut value: Value) -> Result<Restored, FormatError> {
    if let Err(why) = check_chain() {
        return Err(FormatError::Migration { from: OLDEST, why });
    }
    let from = match classify(&value) {
        Classification::Current => {
            return Ok(Restored {
                value,
                migrated_from: None,
            });
        }
        Classification::Malformed { reason } => return Err(FormatError::Malformed(reason)),
        Classification::Unsupported { version } => {
            return Err(FormatError::Unsupported { version });
        }
        Classification::MigrationRequired { from } => from,
    };
    for step in STEPS.iter().filter(|s| s.from >= from) {
        value = (step.apply)(value).map_err(|why| FormatError::Migration {
            from: step.from,
            why,
        })?;
    }
    Ok(Restored {
        value,
        migrated_from: Some(from),
    })
}

/// Stamps the current format on a thread about to be written.
pub fn stamp(value: &mut Value) {
    if let Some(object) = value.as_object_mut() {
        object.insert("version".to_owned(), Value::from(CURRENT));
    }
}

/// Version 0 had no `version` field and nothing else differs.
fn v0_to_v1(mut value: Value) -> Result<Value, String> {
    let object = value.as_object_mut().ok_or("not an object")?;
    object.insert("version".to_owned(), Value::from(1));
    Ok(value)
}

/// Version 1 was the whole thread in one JSON file; version 2 is the same thread kept as a log, so the object is unchanged.
fn v1_to_v2(mut value: Value) -> Result<Value, String> {
    let object = value.as_object_mut().ok_or("not an object")?;
    object.insert("version".to_owned(), Value::from(2));
    Ok(value)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn v0() -> Value {
        json!({ "id": "a", "workspace": "/w", "title": "t", "history": [] })
    }

    #[test]
    fn the_chain_is_unbroken_from_the_oldest_format_to_the_current_one() {
        assert_eq!(check_chain(), Ok(()));
        assert_eq!(u32::try_from(STEPS.len()), Ok(CURRENT - OLDEST));
    }

    #[test]
    fn a_file_says_what_it_is_without_being_restored() {
        assert_eq!(
            classify(&v0()),
            Classification::MigrationRequired { from: 0 }
        );
        assert_eq!(
            classify(&json!({ "id": "a", "workspace": "/w", "history": [], "version": 2 })),
            Classification::Current
        );
        assert_eq!(
            classify(&json!({ "id": "a", "workspace": "/w", "history": [], "version": 9 })),
            Classification::Unsupported { version: 9 }
        );
        for (value, why) in [
            (json!([1]), "not a JSON object"),
            (json!({ "workspace": "/w", "history": [] }), "`id`"),
            (json!({ "id": "a", "history": [] }), "`workspace`"),
            (
                json!({ "id": "a", "workspace": "/w", "history": "no" }),
                "`history`",
            ),
            (
                json!({ "id": "a", "workspace": "/w", "history": [], "version": "one" }),
                "whole number",
            ),
            (
                json!({ "id": "a", "workspace": "/w", "history": [], "version": -1 }),
                "whole number",
            ),
        ] {
            assert!(
                matches!(classify(&value), Classification::Malformed { reason } if reason.contains(why)),
                "{value}"
            );
        }
    }

    #[test]
    fn an_older_file_is_carried_forward_and_its_contents_are_kept() {
        let restored = restore(v0()).expect("restores");
        assert_eq!(restored.migrated_from, Some(0));
        assert_eq!(restored.value["version"], 2, "carried through every step");
        assert_eq!(
            (
                restored.value["title"].as_str(),
                restored.value["id"].as_str()
            ),
            (Some("t"), Some("a"))
        );
    }

    #[test]
    fn a_current_file_comes_back_as_it_is() {
        let current =
            json!({ "id": "a", "workspace": "/w", "history": [], "version": 2, "extra": true });
        let restored = restore(current.clone()).expect("restores");
        assert_eq!((restored.value, restored.migrated_from), (current, None));
    }

    #[test]
    fn a_newer_or_broken_file_is_refused_with_a_reason_and_never_guessed_at() {
        let newer = restore(json!({ "id": "a", "workspace": "/w", "history": [], "version": 3 }))
            .unwrap_err();
        assert_eq!(newer, FormatError::Unsupported { version: 3 });
        assert!(
            newer.to_string().contains("newer Vivianne")
                && newer.to_string().contains("left as it is")
        );
        assert!(matches!(
            restore(json!("text")),
            Err(FormatError::Malformed(_))
        ));
    }

    #[test]
    fn writing_stamps_the_current_format() {
        let mut value = v0();
        stamp(&mut value);
        assert_eq!(classify(&value), Classification::Current);
        let mut not_an_object = json!([1]);
        stamp(&mut not_an_object);
        assert_eq!(not_an_object, json!([1]));
    }
}
