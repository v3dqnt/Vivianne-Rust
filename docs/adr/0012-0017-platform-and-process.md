# ADR 0012-0017: platform and process decisions

**0012 Everything is a plugin.** Evolved from the DeepSeek harness paper. Vivianne's own parts
(providers, tools, memory, agents, UI panels) are compile-time plugins in a shared context: services,
typed events, reversible registrations that unwind when a plugin is removed (`crates/plugins`).
Third-party **micro-apps** are separate processes talking to Vivianne over a local, versioned **app
API** (`crates/app-api`), in any stack. The API exposes headless AI services (agent sessions, tools,
memory, streaming events, model routing) and embeddable panels (a declarative schema Vivianne renders
natively). Headless first, panels second.

**0013 Permissions.** Local-first; the user decides how a micro-app acts. Guardrails:
1. capability declaration and per-app approval
2. writes outside the project ask first
3. no network unless granted
4. token and spend caps per app and session
5. audit log of every call and tool run
6. kill switch

Reuse the per-app consent model from `dip`.

**0014 Existing data.** Threads, sessions, memory (SQLite), settings under `~/.viv` and config folders
open in the native app. Versioned migrations only where a format must change.

**0015 Packaging.** Windows MSI first (macOS later). Signed update manifest checked at launch.
Sign-in stays in the system browser against the existing site with a deep-link return. Tray icon and
global hotkeys rebuilt natively.

**0016 GPUI versioning.** Pin one exact version (it tracks Zed's repo), upgrade deliberately every few
months, keep all GPUI code in the `shell` crate.

**0017 Repo standards.** Small crates, no file over about 600 lines (CI gate), strict clippy in CI,
one ADR per big choice, `cargo test` gates every phase. Old app is kept until parity, then deleted.
Private GitHub repo until parity (name proposed: `vivianne-next`, not yet created).
