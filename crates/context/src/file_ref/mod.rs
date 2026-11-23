//! `@file` mentions: the token grammar and path discovery.

pub mod grammar;
pub mod search;

pub use grammar::{
    ActiveAtToken, Candidate, FILE_REFERENCE_PROMPT, Kind, active_at_token, format_file_mention,
};
pub use search::{Config, SearchError, Stop, WorkspaceSearch};
