//! Durable attachments: images and files a person attaches to a message. Images are checked, normalized and stored
//! under their digest; files are stored byte for byte. A saved thread keeps only references (`vivianne_core::attachment`),
//! so a thread file never holds a path or the bytes, and the same bytes are stored once.
//!
//! The store takes its home folder from the caller. Calls block: run them off the render path.

pub mod admission;
pub mod error;
pub mod files;
pub mod geometry;
pub mod image;
pub mod layout;
pub mod limiter;
pub mod names;
pub mod objects;
pub mod prepare;
pub mod prompt;
pub mod store;

pub use error::{AttachmentError, Code, Error, Result};
pub use geometry::{long_edge_dimensions, request_image_dimensions};
pub use limiter::Cancel;
pub use prepare::{Limits, SaveImage, StoredImage};
pub use prompt::{Admitted, PromptPart};
pub use store::{Config, Store};
