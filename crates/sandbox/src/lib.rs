//! Starting other programs and keeping hold of them (the original `subprocess`), and confining what they may do (the
//! original `sandbox`). The only crate besides the shell's window glue that may use `unsafe`, for the OS calls each
//! of those needs; every such block says why it is sound. Gaps: `docs/subprocess-gaps.md`.

pub mod env;
pub mod output;
pub mod spawn;
pub mod tree;
