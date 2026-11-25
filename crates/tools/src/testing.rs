//! What the tests of this crate share: a new, empty folder to work in. (Each test removes its own when it is done.)

use std::path::PathBuf;

/// A new, empty folder under the system temp folder, named `viv-<what>-<unique>`.
pub(crate) fn scratch(what: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("viv-{what}-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).expect("a scratch folder can be made");
    dir
}
