//! The values the Settings pages edit. Held in memory only: nothing is saved
//! or applied yet. Saving into `~/.viv` (the format the old app wrote) comes
//! with the settings store.

use std::cell::RefCell;
use std::rc::Rc;

// One flag per switch on the Settings pages, so many bools is the point.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone)]
pub struct Values {
    pub retry_transient: bool,
    pub search_provider: String,
    pub remember_across_sessions: bool,
    pub auto_consolidate: bool,
    pub auto_extract: bool,
    pub refuse_secrets: bool,
    pub share_diagnostics: bool,
    pub chat_density: String,
    pub chat_width: String,
    pub sidebar_side: String,
}

impl Default for Values {
    /// Defaults mirror the old app's.
    fn default() -> Self {
        Self {
            retry_transient: true,
            search_provider: "auto".to_owned(),
            remember_across_sessions: true,
            auto_consolidate: true,
            auto_extract: true,
            refuse_secrets: true,
            share_diagnostics: false,
            chat_density: "comfortable".to_owned(),
            chat_width: "standard".to_owned(),
            sidebar_side: "left".to_owned(),
        }
    }
}

/// The values, shared between the page builders and the field callbacks.
pub type Shared = Rc<RefCell<Values>>;
