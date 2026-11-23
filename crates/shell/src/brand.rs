//! The mark of an app, in full colour. Where one comes from, first found wins:
//! 1. `~/.viv/brands/<id>.svg` (or `.png`): a logo put there by hand or exported from the React app (see
//!    `scripts/export-react-logos.mjs`). Drop a file in and it is used the next time it is drawn, no rebuild.
//! 2. The site's own icon, fetched once from its domain and kept in `~/.viv/icons/`.
//! 3. A tile with the name's first letter, so a row of apps always lines up.
//!
//! The pictures are drawn as they are (an SVG keeps its colours), on a quiet rounded tile.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;

use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, IntoElement, ObjectFit, Styled, div, img, px};

use crate::theme::{self, color};

/// Domains whose icon is being fetched or has failed, so each is tried once per run.
static TRIED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn brands_dir() -> PathBuf {
    vivianne_ai::paths::viv_home_path(&["brands"])
}

fn icons_dir() -> PathBuf {
    vivianne_ai::paths::viv_home_path(&["icons"])
}

/// The file to draw for this app, if one is kept.
fn find(id: &str, domain: &str) -> Option<PathBuf> {
    let safe = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    };
    if safe(id) {
        for ext in ["svg", "png"] {
            let path = brands_dir().join(format!("{id}.{ext}"));
            if path.is_file() {
                return Some(path);
            }
        }
    }
    if safe(domain) {
        let path = icons_dir().join(format!("{domain}.png"));
        // A failed download can leave a tiny or empty file: that is not an icon.
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > 200) {
            return Some(path);
        }
    }
    None
}

/// The site a provider, coding agent or model maker's icon comes from, by the id the app uses for it.
#[must_use]
pub fn domain_for(id: &str) -> Option<&'static str> {
    Some(match id.to_ascii_lowercase().as_str() {
        "anthropic" | "claude" | "claude-acp" | "claude-code" => "claude.ai",
        "openai" | "codex" | "codex-acp" | "chatgpt" => "openai.com",
        "google" => "google.com",
        "gemini" | "gemini-cli" => "gemini.google.com",
        "groq" => "groq.com",
        "xai" | "grok" | "grok-build" => "x.ai",
        "deepseek" => "deepseek.com",
        "mistral" => "mistral.ai",
        "together" => "together.ai",
        "fireworks" => "fireworks.ai",
        "openrouter" => "openrouter.ai",
        "cerebras" => "cerebras.ai",
        "nvidia" | "nvidia-nim" => "nvidia.com",
        "nebius" => "nebius.com",
        "huggingface" => "huggingface.co",
        "cloudflare" | "cloudflare-workers-ai" => "cloudflare.com",
        "github-copilot" | "github-copilot-cli" | "copilot" | "copilot-acp" => "github.com",
        "ollama" => "ollama.com",
        "meta" => "meta.com",
        "perplexity" => "perplexity.ai",
        "windsurf" => "windsurf.com",
        "qwen" | "qwen-code" => "qwen.ai",
        "cursor" => "cursor.com",
        "opencode" => "opencode.ai",
        "goose" => "block.xyz",
        "cline" => "cline.bot",
        "kimi" | "moonshot" => "moonshot.ai",
        _ => return None,
    })
}

/// Fetches the icons for these sites, one after another and once each, redrawing as they arrive.
pub fn prefetch(cx: &mut App, domains: Vec<String>) {
    let wanted: Vec<String> = {
        let mut tried = TRIED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let tried = tried.get_or_insert_with(HashSet::new);
        domains
            .into_iter()
            .filter(|d| !d.is_empty() && find("", d).is_none() && tried.insert(d.clone()))
            .collect()
    };
    if wanted.is_empty() {
        return;
    }
    crate::backend::run(
        cx,
        async move {
            let mut got = 0usize;
            for domain in wanted {
                if vivianne_tools::connections::icon_file(&domain).await.is_ok() {
                    got += 1;
                }
            }
            got
        },
        |got, cx| {
            if got > 0 {
                cx.refresh_windows();
            }
        },
    );
}

/// The sites of every provider and agent the app knows, for `prefetch`.
#[must_use]
pub fn known_domains() -> Vec<String> {
    let mut all: Vec<String> = [
        "anthropic", "openai", "google", "gemini", "groq", "xai", "deepseek", "mistral", "together",
        "fireworks", "openrouter", "cerebras", "nvidia", "nebius", "huggingface", "cloudflare",
        "github-copilot", "ollama", "meta", "perplexity", "windsurf", "qwen", "cursor", "opencode",
        "goose", "cline", "kimi",
    ]
    .iter()
    .filter_map(|id| domain_for(id).map(str::to_owned))
    .collect();
    all.sort();
    all.dedup();
    all
}

/// The mark of a provider, agent or model maker by the app's id for it.
pub fn tile(id: &str, name: &str, size: f32) -> AnyElement {
    tile_with(id, domain_for(id).unwrap_or(""), name, size)
}

/// The mark in a `size` square tile: the picture if there is one, else the bundled mark or a letter.
pub fn tile_with(id: &str, domain: &str, name: &str, size: f32) -> AnyElement {
    match find(id, domain) {
        Some(path) => div()
            .flex_none()
            .size(px(size))
            .rounded(px(size * 0.26))
            .bg(color(theme::RAISED))
            .p(px(size * 0.18))
            .overflow_hidden()
            .child(img(path).size_full().object_fit(ObjectFit::Contain))
            .into_any_element(),
        None => crate::settings::logos::tile(id, name, size).into_any_element(),
    }
}
