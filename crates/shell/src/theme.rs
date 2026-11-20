//! Design tokens: one dark palette, one blue accent, two radii, a short type scale.
//!
//! Structure comes from layout and type, not from boxes: the side panels sit one step in front of the window
//! (`PANEL`) behind a hairline, sections inside them are separated by space and a hairline, not by cards. Depth,
//! from back to front: the window (`BG`), the side panels (`PANEL`), bubbles and the composer (`SURFACE`), and
//! what the pointer is over (`RAISED`). Text comes in three strengths: `TEXT`, `MUTED` (secondary, 6:1) and
//! `FAINT` (labels and hints only, never a sentence). The accent is kept for what is selected or primary.
//! Radii: 6 px for controls, 10 px for surfaces. Dark only for now.
//!
//! [`apply`] pushes the same tokens into Kit's theme so its components (text
//! input, markdown, buttons, dialogs) match the parts we draw ourselves.

// Colours are written as the usual six-digit hex, not 0x0011_1214.
#![allow(clippy::unreadable_literal)]

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, Hsla, Rgba, px, rgb};

/// App background.
pub const BG: u32 = 0x0e1014;
/// The side panels (threads on the left, the project on the right): one step in front of the window.
pub const PANEL: u32 = 0x121419;
/// Raised surfaces: bubbles, the composer, menus.
pub const SURFACE: u32 = 0x1a1d24;
/// What the pointer is over, and what is selected in a list.
pub const RAISED: u32 = 0x22262f;
/// Recessed areas: code, tool output.
pub const CODE_BG: u32 = 0x0a0c0f;
/// Edges of controls and surfaces.
pub const BORDER: u32 = 0x2a2f3a;
/// The edge of what has focus: one step lighter than `BORDER`.
pub const FOCUS_EDGE: u32 = 0x404757;
/// Lines that divide areas: quieter than a control's edge.
pub const HAIRLINE: u32 = 0x1e2129;
pub const TEXT: u32 = 0xe6e8ec;
/// Secondary text, 6:1 on `BG`.
pub const MUTED: u32 = 0x8b93a1;
/// Labels and hints, never a sentence someone must read: 3.4:1 on `BG`.
pub const FAINT: u32 = 0x626a78;
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
/// The side panel: a card that floats beside the chat.
pub const RADIUS_PANEL: f32 = 16.0;
/// The composer, the main thing you touch: rounder than a card, not a pill.
/// GPUI has no corner smoothing, so this is a plain circular arc.
pub const RADIUS_COMPOSER: f32 = 14.0;

/// Gap between transcript rows.
pub const GAP: f32 = 18.0;
/// Side padding of the transcript and composer.
pub const SIDE_PAD: f32 = 32.0;
/// Reading width of the chat column: about 75 characters of body text.
pub const COLUMN_MAX: f32 = 720.0;

/// The type scale. Hierarchy comes from weight and colour first, size second.
pub const TEXT_XS: f32 = 11.0;
pub const TEXT_SM: f32 = 12.5;
pub const TEXT_BASE: f32 = 14.0;
pub const TEXT_LG: f32 = 16.0;
pub const TEXT_TITLE: f32 = 24.0;

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
        colors.title_bar = hsla(BG);
        colors.title_bar_border = hsla(HAIRLINE);
        colors.sidebar = hsla(PANEL);
        colors.sidebar_border = hsla(HAIRLINE);
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
