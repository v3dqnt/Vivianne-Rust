//! Design tokens: one dark palette with four levels of depth, one blue accent, two radii.
//!
//! The greys lean slightly blue so the app has a temperature instead of looking flat. Depth, from back to front:
//! the window (`BG`), the side panel and cards (`PANEL`), bubbles and the composer (`SURFACE`), and what the
//! pointer is over (`RAISED`). The accent is used sparingly (the active tab, the send button, your messages,
//! links), so it reads as meaning something. Off-white text, no pure black. Radii follow the old component library (`viv-ui`): 6 px for controls,
//! 10 px for surfaces. Dark only for now.
//!
//! [`apply`] pushes the same tokens into Kit's theme so its components (text
//! input, markdown, buttons, dialogs) match the parts we draw ourselves.

// Colours are written as the usual six-digit hex, not 0x0011_1214.
#![allow(clippy::unreadable_literal)]

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, Hsla, Rgba, px, rgb};

/// App background.
pub const BG: u32 = 0x0f1115;
/// The side panel and cards: one step in front of the window.
pub const PANEL: u32 = 0x151820;
/// Raised surfaces: bubbles, rows, the composer.
pub const SURFACE: u32 = 0x1b1f28;
/// What the pointer is over.
pub const RAISED: u32 = 0x242a36;
/// Recessed areas: code, tool output, title bar.
pub const CODE_BG: u32 = 0x0b0d11;
pub const BORDER: u32 = 0x2a303c;
pub const TEXT: u32 = 0xe9ebf0;
/// Secondary text, 6:1 on `BG`.
pub const MUTED: u32 = 0x959dad;
/// The one accent.
pub const ACCENT: u32 = 0x5b8cff;
pub const ACCENT_HOVER: u32 = 0x7aa0ff;
const ACCENT_ACTIVE: u32 = 0x4673e0;
/// The accent faintly over the window: the fill of your messages and of selected things.
pub const ACCENT_SOFT: u32 = 0x1a2745;
/// The border that goes with `ACCENT_SOFT`.
pub const ACCENT_LINE: u32 = 0x2f4780;
/// Text on `ACCENT_SOFT`: the accent lightened to read well on it.
pub const ACCENT_TEXT: u32 = 0xa9c1ff;
/// Text and icons on the accent.
pub const ON_ACCENT: u32 = 0xffffff;
pub const OK: u32 = 0x45c179;
pub const DANGER: u32 = 0xf0605a;
/// Something worth a look but not wrong.
pub const WARN: u32 = 0xe3a948;

/// Buttons and inputs.
pub const RADIUS_CONTROL: f32 = 6.0;
/// Cards, bubbles, code blocks.
pub const RADIUS_SURFACE: f32 = 10.0;
/// The composer, the main thing you touch: rounder than a card, not a pill.
/// GPUI has no corner smoothing, so this is a plain circular arc.
pub const RADIUS_COMPOSER: f32 = 16.0;

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
        colors.secondary_hover = hsla(RAISED);
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
