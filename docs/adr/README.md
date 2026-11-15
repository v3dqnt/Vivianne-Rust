# Architecture decisions

One note per big choice. Source: the 2026-11-15 grilling session. Status of all: accepted.

| # | Decision |
|---|---|
| 0001 | Why: speed, memory, one language, native feel |
| 0002 | Target: fully native GPU UI with GPUI, no webview |
| 0003 | Look: Fluent on Windows first, liquid glass on macOS later |
| 0004 | Rollout in phases; old Tauri app lives until parity |
| 0005 | Drop Tauri; backend is called in-process |
| 0006 | Refactor backend together with the frontend (includes agent merge) |
| 0007 | Replace generated provider layer with small adapters behind one trait |
| 0008 | Old web UI and HeroUI work frozen to bug fixes |
| 0009 | Verification: GPUI gallery binary plus screenshots |
| 0010 | Rich content: custom 3D robot heads, mermaid and formulas deferred |
| 0011 | Old HTML/iframe mods dropped, replaced by micro-apps |
| 0012 | Everything is a plugin; micro-apps are separate processes on an app API |
| 0013 | Micro-app permissions: local-first, six guardrails |
| 0014 | Existing user data stays readable |
| 0015 | Packaging without Tauri: MSI, signed update manifest, browser sign-in |
| 0016 | Pin one exact GPUI version, keep GPUI code in the shell crate |
| 0017 | Repo standards: small crates, files under ~600 lines, strict clippy, tests gate phases |
