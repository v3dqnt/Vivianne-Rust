//! Which model does the work and what it costs: how hard a task is, cost-aware routing to a quick
//! model, getting open models onto this computer, the local `llama-server`, and recorded token usage.

pub mod difficulty;
pub mod llama_manager;
pub mod local_models;
pub mod routing;
pub mod usage_store;
