//! A company or app mark beside a name: the logo when the app has one, otherwise the name's first letter on a tile of
//! a colour chosen from the name (so the same name always looks the same). The mark sits on a quiet rounded tile so a
//! row of them lines up whether or not each has a real logo.

use gpui_kit::component::Icon;
use gpui_kit::prelude::*;
use gpui_kit::{FontWeight, IntoElement, Styled, div, px, rgb};

use crate::theme::{self, color};

/// The logo file for a provider or coding agent id, when there is one. Ids are the app's own (`nvidia-nim`,
/// `claude-acp`); a company whose mark is not in the set gets `None` and so a monogram.
#[must_use]
pub fn mark_for(id: &str) -> Option<&'static str> {
    let id = id.to_ascii_lowercase();
    let found = match id.as_str() {
        "anthropic" | "claude" | "claude-acp" | "claude-code" => "claude",
        "openai" | "codex" | "codex-acp" => "openai",
        "google" => "google",
        "gemini" | "gemini-cli" => "googlegemini",
        "nvidia-nim" | "nvidia" => "nvidia",
        "huggingface" => "huggingface",
        "cloudflare-workers-ai" | "cloudflare" => "cloudflare",
        "github-copilot" | "copilot" | "copilot-acp" => "githubcopilot",
        "ollama" => "ollama",
        "meta" => "meta",
        "perplexity" => "perplexity",
        "windsurf" => "windsurf",
        "qwen" | "qwen-code" | "alibaba" => "alibabacloud",
        _ => return None,
    };
    Some(found)
}

/// The company behind a model, from its name (`Opus 4.5` is Anthropic's, `GPT-5` OpenAI's): an id `mark_for` knows,
/// or `None` for a model whose maker is not recognised.
#[must_use]
pub fn model_company(model: &str) -> Option<&'static str> {
    let m = model.to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| m.contains(w));
    if has(&["claude", "opus", "sonnet", "haiku"]) {
        Some("anthropic")
    } else if has(&["gpt", "codex", "o3", "o4-mini", "chatgpt"]) {
        Some("openai")
    } else if has(&["gemini", "gemma"]) {
        Some("gemini")
    } else if has(&["llama"]) {
        Some("meta")
    } else if has(&["qwen"]) {
        Some("qwen")
    } else if has(&["nemotron", "nvidia"]) {
        Some("nvidia")
    } else {
        None
    }
}

/// A colour for a name's monogram: stable, and muted enough not to compete with the text.
fn tone(name: &str) -> u32 {
    const TONES: [u32; 8] = [
        0x3b5b8a, 0x5b4b8a, 0x8a4b6b, 0x8a5b3b, 0x4b7a5b, 0x3b7a82, 0x7a6b3b, 0x6b4b3b,
    ];
    let sum = name.bytes().fold(0usize, |acc, b| {
        acc.wrapping_mul(31).wrapping_add(usize::from(b))
    });
    TONES[sum % TONES.len()]
}

/// The mark in a `size` square tile.
pub fn tile(id: &str, name: &str, size: f32) -> impl IntoElement {
    let base = div()
        .flex_none()
        .size(px(size))
        .rounded(px(size * 0.26))
        .flex()
        .items_center()
        .justify_center()
        .overflow_hidden();
    match mark_for(id).and_then(crate::assets::logo_path) {
        Some(path) => base
            .bg(color(theme::RAISED))
            .text_color(color(theme::TEXT))
            .child(Icon::empty().path(path).size(px(size * 0.58))),
        None => base
            .bg(rgb(tone(name)))
            .text_size(px(size * 0.5))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(color(theme::ON_ACCENT))
            .child(
                name.chars()
                    .next()
                    .map_or_else(|| "?".to_owned(), |c| c.to_uppercase().collect()),
            ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_companies_and_agents_have_a_logo_and_others_get_a_monogram() {
        for id in [
            "anthropic",
            "openai",
            "google",
            "nvidia-nim",
            "huggingface",
            "claude-acp",
            "codex-acp",
            "gemini",
            "github-copilot",
        ] {
            assert!(
                mark_for(id).and_then(crate::assets::logo_path).is_some(),
                "{id}"
            );
        }
        for id in ["groq", "deepseek", "nebius", "some-new-agent"] {
            assert!(mark_for(id).is_none(), "{id}");
        }
    }

    #[test]
    fn a_models_maker_is_read_from_its_name() {
        assert_eq!(model_company("Opus 4.5"), Some("anthropic"));
        assert_eq!(model_company("gpt-5-codex"), Some("openai"));
        assert_eq!(model_company("Gemini 2.5 Pro"), Some("gemini"));
        assert_eq!(model_company("nvidia/nemotron-3-ultra"), Some("nvidia"));
        assert_eq!(model_company("mystery-model"), None);
    }

    #[test]
    fn a_names_monogram_colour_never_changes() {
        assert_eq!(tone("Groq"), tone("Groq"));
        assert!(
            (0..8).contains(
                &[
                    0x3b5b8a, 0x5b4b8a, 0x8a4b6b, 0x8a5b3b, 0x4b7a5b, 0x3b7a82, 0x7a6b3b, 0x6b4b3b
                ]
                .iter()
                .position(|t| *t == tone("DeepSeek"))
                .unwrap_or(99)
            )
        );
    }
}
