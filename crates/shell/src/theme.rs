//! Design tokens in the manner of Apple's dark interface: a black base, grouped greys that step forward, system blue,
//! large continuous-feeling radii and an iOS-like type scale. (From Apple's published human interface guidelines and
//! system colours, not from a design file; the system font is the platform's own, never an embedded Apple font.)
//!
//! Structure comes from layout and type, not from boxes: the side panels sit one step in front of the window
//! (`PANEL`) behind a hairline, sections inside them are separated by space and a hairline, not by cards. Depth,
//! from back to front: the window (`BG`), the side panels (`PANEL`), bubbles and the composer (`SURFACE`), and
//! what the pointer is over (`RAISED`). Text comes in three strengths: `TEXT`, `MUTED` (secondary, 6:1) and
//! `FAINT` (labels and hints only, never a sentence). The accent is kept for what is selected or primary.
//! Radii: 10 px for controls, 16 for surfaces, 20 for panels, 24 for the composer. Dark only for now.
//!
//! [`apply`] pushes the same tokens into Kit's theme so its components (text
//! input, markdown, buttons, dialogs) match the parts we draw ourselves.

// Colours are written as the usual six-digit hex, not 0x0011_1214.
#![allow(clippy::unreadable_literal)]

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, Hsla, Rgba, px, rgb};

/// App background.
pub const BG: u32 = 0x000000;
/// The side panels (threads on the left, the project on the right): one step in front of the window.
pub const PANEL: u32 = 0x111113;
/// Raised surfaces: bubbles, the composer, menus.
pub const SURFACE: u32 = 0x1c1c1e;
/// What the pointer is over, and what is selected in a list.
pub const RAISED: u32 = 0x2c2c2e;
/// Recessed areas: code, tool output.
pub const CODE_BG: u32 = 0x0a0a0a;
/// Edges of controls and surfaces.
pub const BORDER: u32 = 0x38383a;
/// The edge of what has focus: one step lighter than `BORDER`.
pub const FOCUS_EDGE: u32 = 0x545458;
/// Lines that divide areas: quieter than a control's edge.
pub const HAIRLINE: u32 = 0x2a2a2c;
pub const TEXT: u32 = 0xffffff;
/// Secondary text, 6:1 on `BG`.
pub const MUTED: u32 = 0x99999e;
/// Labels and hints, never a sentence someone must read: 4.7:1 on `BG` (WCAG AA for small text).
pub const FAINT: u32 = 0x7c7c80;
/// The one accent.
pub const ACCENT: u32 = 0x0a84ff;
pub const ACCENT_HOVER: u32 = 0x409cff;
const ACCENT_ACTIVE: u32 = 0x0a6fe0;
/// The accent faintly over the window: the fill of your messages and of selected things.
pub const ACCENT_SOFT: u32 = 0x0b2540;
/// The border that goes with `ACCENT_SOFT`.
pub const ACCENT_LINE: u32 = 0x1c5fb8;
/// Text on `ACCENT_SOFT`: the accent lightened to read well on it.
pub const ACCENT_TEXT: u32 = 0x64b2ff;
/// Text and icons on the accent.
pub const ON_ACCENT: u32 = 0xffffff;
pub const OK: u32 = 0x30d158;
pub const DANGER: u32 = 0xff453a;
/// Something worth a look but not wrong.
pub const WARN: u32 = 0xff9f0a;

/// The selected segment of a segmented control: one step above `RAISED`.
pub const SEGMENT: u32 = 0x3a3a3c;

/// Buttons and inputs.
pub const RADIUS_CONTROL: f32 = 10.0;
/// Cards, bubbles, code blocks.
pub const RADIUS_SURFACE: f32 = 16.0;
/// The side panel: a card that floats beside the chat.
pub const RADIUS_PANEL: f32 = 20.0;
/// The composer, the main thing you touch: rounder than a card, not a pill.
/// GPUI has no corner smoothing, so this is a plain circular arc.
pub const RADIUS_COMPOSER: f32 = 24.0;

/// Gap between transcript rows.
pub const GAP: f32 = 18.0;
/// Side padding of the transcript and composer.
pub const SIDE_PAD: f32 = 32.0;
/// Reading width of the chat column: about 75 characters of body text.
pub const COLUMN_MAX: f32 = 720.0;

/// The type scale. Hierarchy comes from weight and colour first, size second.
pub const TEXT_XS: f32 = 12.0;
pub const TEXT_SM: f32 = 13.0;
pub const TEXT_BASE: f32 = 15.0;
pub const TEXT_LG: f32 = 17.0;
pub const TEXT_TITLE: f32 = 28.0;

pub const MONO: &str = if cfg!(windows) { "Consolas" } else { "Menlo" };
/// The app's typeface (see `assets`); the platform's own font fills in for a character it does not have.
const UI_FONT: &str = crate::assets::FONT_FAMILY;

#[must_use]
/// The frosted-glass tint of the header bar and the rail: dark and translucent, over the blurred window.
#[must_use]
pub fn glass() -> Rgba {
    gpui_kit::rgba(0x141416a6)
}

/// A faint lift over glass for controls (the tab track, the selected pill).
#[must_use]
pub fn glass_lift(alpha: u8) -> Rgba {
    gpui_kit::rgba(0xffffff00 | u32::from(alpha))
}

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
        // A sidebar that is part of what it sits in (the Settings dialog's list, for one): the same surface, a quiet
        // line between it and the page, and a soft fill under the page that is open.
        // Dialogs (Settings) are a raised sheet over a dimmed window, as Apple's sheets are.
        colors.overlay = Hsla {
            a: 0.55,
            ..hsla(0x000000)
        };
        colors.sidebar = hsla(SURFACE);
        colors.sidebar_border = hsla(HAIRLINE);
        colors.sidebar_foreground = hsla(MUTED);
        colors.sidebar_accent = hsla(RAISED);
        colors.sidebar_accent_foreground = hsla(TEXT);
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
