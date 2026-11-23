//! How smoothly the whole window is drawing. The root view notes every frame it draws here, so the numbers are
//! the real pace of the app in use (typing, scrolling, a streaming reply), not of one screen.
//!
//! GPUI draws only when something changed and, on Windows, waits for the display's own refresh (the DWM compositor
//! clock) before each frame: a 120 or 144 Hz screen is drawn at 120 or 144 Hz as long as a frame takes less than
//! the refresh period. There is no frame cap to lift. What this measures is whether frames fit that period.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::PoisonError;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// Frames kept.
const KEPT: usize = 480;
/// A gap longer than this is the app sitting idle (nothing changed, so nothing was drawn), not a slow frame.
const IDLE_GAP_MS: f32 = 100.0;

struct Clock {
    last: Option<Instant>,
    intervals: VecDeque<f32>,
}

static CLOCK: Mutex<Clock> = Mutex::new(Clock {
    last: None,
    intervals: VecDeque::new(),
});
static CONTINUOUS: AtomicBool = AtomicBool::new(false);

/// Notes that a frame was drawn now. Called by the root view on every frame.
pub fn record_frame() {
    let now = Instant::now();
    let mut clock = CLOCK.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(last) = clock.last {
        let ms = now.duration_since(last).as_secs_f32() * 1000.0;
        if ms < IDLE_GAP_MS {
            if clock.intervals.len() == KEPT {
                clock.intervals.pop_front();
            }
            clock.intervals.push_back(ms);
        }
    }
    clock.last = Some(now);
}

/// The frame intervals in milliseconds, oldest first.
#[must_use]
pub fn intervals() -> Vec<f32> {
    CLOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .intervals
        .iter()
        .copied()
        .collect()
}

pub fn reset() {
    let mut clock = CLOCK.lock().unwrap_or_else(PoisonError::into_inner);
    clock.last = None;
    clock.intervals.clear();
}

/// Whether the whole window redraws continuously (the Debug tab's switch), to measure what a frame costs when
/// nothing else is asking for one.
#[must_use]
pub fn continuous() -> bool {
    CONTINUOUS.load(Ordering::Relaxed)
}

pub fn set_continuous(on: bool) {
    CONTINUOUS.store(on, Ordering::Relaxed);
    reset();
}
