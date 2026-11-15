//! Design tokens: one calm dark palette, one blue accent, two radii.
//!
//! Neutral greys with a faint cool tint, off-white text, no glows and no pure
//! black. Radii follow the old component library (`viv-ui`): 6 px for controls,
//! 10 px for surfaces. Dark only for now.
//!
//! [`apply`] pushes the same tokens into Kit's theme so its components (text
//! input, markdown, buttons, dialogs) match the parts we draw ourselves.

// Colours are written as the usual six-digit hex, not 0x0011_1214.
#![allow(clippy::unreadable_literal)]

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, Hsla, Rgba, px, rgb};

/// App background.
pub const BG: u32 = 0x111214;
/// Raised surfaces: bubbles, rows, the composer.
pub const SURFACE: u32 = 0x1a1c20;
/// Recessed areas: code, tool output, title bar.
pub const CODE_BG: u32 = 0x0c0d0f;
pub const BORDER: u32 = 0x25282d;
pub const TEXT: u32 = 0xe7e8ea;
/// Secondary text, 6:1 on `BG`.
pub const MUTED: u32 = 0x8e94a0;
/// The one accent.
pub const ACCENT: u32 = 0x4a82e6;
const ACCENT_HOVER: u32 = 0x6a98ec;
const ACCENT_ACTIVE: u32 = 0x3b6dc7;
const ON_ACCENT: u32 = 0xffffff;
pub const OK: u32 = 0x3fb56b;
pub const DANGER: u32 = 0xe5534b;

/// Buttons and inputs.
pub const RADIUS_CONTROL: f32 = 6.0;
/// Cards, bubbles, code blocks.
pub const RADIUS_SURFACE: f32 = 10.0;

/// Gap between transcript rows.
pub const GAP: f32 = 14.0;
/// Side padding of the transcript and composer.
pub const SIDE_PAD: f32 = 28.0;
/// Reading width of the chat column.
pub const COLUMN_MAX: f32 = 780.0;

pub const MONO: &str = if cfg!(windows) { "Consolas" } else { "Menlo" };
const UI_FONT: &str = if cfg!(windows) {
    "Segoe UI Variable Text"
} else {
    ".SystemUIFont"
};

#[must_use]
pub fn color(hex: u32) -> Rgba {
    rgb(hex)
}

fn hsla(hex: u32) -> Hsla {
    Hsla::from(rgb(hex))
}

/// Switch Kit to dark and override its tokens with ours.
pub fn apply(cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);
    Theme::update(cx, |theme| {
        let colors = &mut theme.colors;
        colors.background = hsla(BG);
        colors.foreground = hsla(TEXT);
        colors.border = hsla(BORDER);
        colors.input = hsla(BORDER);
        colors.muted = hsla(CODE_BG);
        colors.muted_foreground = hsla(MUTED);
        colors.secondary = hsla(SURFACE);
        colors.popover = hsla(SURFACE);
        colors.ring = hsla(ACCENT);
        colors.title_bar = hsla(CODE_BG);
        colors.title_bar_border = hsla(BORDER);
        colors.sidebar = hsla(CODE_BG);
        colors.sidebar_border = hsla(BORDER);
        colors.primary = hsla(ACCENT);
        colors.primary_hover = hsla(ACCENT_HOVER);
        colors.primary_active = hsla(ACCENT_ACTIVE);
        colors.primary_foreground = hsla(ON_ACCENT);
        colors.button_primary = hsla(ACCENT);
        colors.button_primary_hover = hsla(ACCENT_HOVER);
        colors.button_primary_active = hsla(ACCENT_ACTIVE);
        colors.button_primary_foreground = hsla(ON_ACCENT);
        colors.danger = hsla(DANGER);
        colors.success = hsla(OK);
        theme.radius = px(RADIUS_CONTROL);
        theme.radius_lg = px(RADIUS_SURFACE);
        theme.font_family = UI_FONT.into();
        theme.mono_font_family = MONO.into();
    });
}
