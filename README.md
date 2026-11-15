# Vivianne.rs

Native Rust rewrite of Vivianne (GPUI UI, no webview, no Tauri).

| Folder | What |
|---|---|
| `crates/` | The Cargo workspace members |
| `docs/` | Architecture and decision records |
| `.github/` | CI |

Gate for every change: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.

Rule: every folder has a `README.md` (CI enforces it).
