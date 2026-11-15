//! Design tokens. Values come from the old app's component library (`viv-ui`):
//! radius tiers (6 px controls, 10 px surfaces) and the dark palette. Dark only for now.

// Colours are written as the usual six-digit hex, not 0x0014_161a.
#![allow(clippy::unreadable_literal)]

use gpui::{Rgba, rgb};

pub const BG: u32 = 0x14161a;
pub const SURFACE: u32 = 0x1b1e24;
pub const CODE_BG: u32 = 0x101216;
pub const BORDER: u32 = 0x2a2e36;
pub const TEXT: u32 = 0xe6e6e6;
pub const MUTED: u32 = 0x8b93a1;
pub const ACCENT: u32 = 0x3b82f6;
pub const OK: u32 = 0x22c55e;
pub const DANGER: u32 = 0xef4444;

/// Buttons and inputs.
pub const RADIUS_CONTROL: f32 = 6.0;
/// Cards, bubbles, code blocks.
pub const RADIUS_SURFACE: f32 = 10.0;

/// Gap between transcript rows.
pub const GAP: f32 = 12.0;
/// Side padding of the transcript and composer.
pub const SIDE_PAD: f32 = 26.0;
/// Reading width of the chat column.
pub const COLUMN_MAX: f32 = 820.0;

pub const MONO: &str = if cfg!(windows) { "Consolas" } else { "Menlo" };

#[must_use]
pub fn color(hex: u32) -> Rgba {
    rgb(hex)
}
