//! Vivianne's AI layer, brought over from the old app: the provider registry, a streaming client for
//! each kind of provider, credentials, model discovery, one-shot completions, embeddings, image
//! generation, and the Laya and Jev decision engines.
//!
//! `ai/` is that code. `inference/` is the rest of the old model layer: routing, local models, the llama
//! server and usage. Around them, the only things it needs from the app: [`events`] (where stream
//! tokens and refresh notices go), [`paths`] (the data folder) and [`host`] (settings, keys, downloads).

#[rustfmt::skip]
pub mod ai;
pub mod events;
pub mod host;
pub mod inference;
pub mod paths;
