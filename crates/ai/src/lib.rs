//! The provider layer from the old Vivianne, copied: the provider registry, the
//! streaming API for each kind of provider, credentials and model discovery.
//!
//! `ai/` is that code. Beside it are the only things it needs from the host:
//! [`events`] (where stream tokens and refresh notices go) and [`paths`] (the
//! data folder). See the README for exactly what was changed in the copy.

// The copied code keeps places where Tauri calls used to be (an unused argument)
// and helpers only the old app's commands called. Left as copied.
#![allow(unused, dead_code)]

#[rustfmt::skip]
pub mod ai;
pub mod events;
pub mod paths;
