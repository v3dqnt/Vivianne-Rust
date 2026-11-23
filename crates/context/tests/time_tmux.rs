//! The clock and the terminal location: when a reading is due, what it says, and what it refuses.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::pedantic)]

use chrono_tz::Tz;
use vivianne_context::time::{
    Clock, RequestZone, ZoneError, canonical_zone, derive_request_zone, format_duration,
    format_timestamp, render_reading, turn_of,
};
use vivianne_context::tmux::{self, Location, Runner};
use vivianne_core::context::ContextSource;
use vivianne_core::{Message, Role};

fn typed(text: &str, at: u64) -> Message {
    Message::new(Role::User, text).at(at)
}

#[test]
fn only_exact_iana_spellings_are_zones() {
    assert!(canonical_zone("Asia/Kolkata").is_ok() && canonical_zone("UTC").is_ok());
    assert_eq!(canonical_zone("").unwrap_err(), ZoneError::Blank);
    assert_eq!(canonical_zone(" UTC").unwrap_err(), ZoneError::Padded);
    assert_eq!(
        canonical_zone("Mars/Base").unwrap_err(),
        ZoneError::Unsupported
    );
    assert!(
        canonical_zone("asia/kolkata").is_err(),
        "a case variant is refused, whichever way"
    );
    assert_ne!(ZoneError::Blank, ZoneError::NotCanonical);
}

#[test]
fn a_request_has_one_zone_many_or_none() {
    let s = |v: &[&str]| v.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
    assert_eq!(derive_request_zone(&s(&[])).unwrap(), RequestZone::Missing);
    assert_eq!(
        derive_request_zone(&s(&["UTC", "UTC"])).unwrap(),
        RequestZone::Resolved("UTC".into())
    );
    assert_eq!(
        derive_request_zone(&s(&["UTC", "Asia/Tokyo"])).unwrap(),
        RequestZone::Mixed(s(&["Asia/Tokyo", "UTC"]))
    );
    assert!(
        derive_request_zone(&s(&["UTC", "nonsense"])).is_err(),
        "one bad zone refuses the whole request"
    );
}

#[test]
fn time_is_written_with_local_fields_offset_and_zone() {
    let kolkata: Tz = "Asia/Kolkata".parse().unwrap();
    assert_eq!(
        format_timestamp(0, kolkata),
        "1970-01-01T05:30:00+05:30[Asia/Kolkata]"
    );
    assert_eq!(
        format_timestamp(0, chrono_tz::UTC),
        "1970-01-01T00:00:00+00:00[UTC]"
    );
    assert_eq!(format_duration(93_784_000), "1d 2h 3m 4s");
    assert_eq!(format_duration(999), "0s");
    assert_eq!(
        format_duration(-5000),
        "0s",
        "a clock that went backward reads as zero"
    );
}

#[test]
fn the_reading_names_the_turn_the_step_and_what_the_elapsed_time_is_since() {
    let first = render_reading(
        10_000,
        2,
        1,
        Some(4_000),
        chrono_tz::UTC,
        &RequestZone::Missing,
    );
    assert!(
        first.contains("turn 2, step 1")
            && first.contains("preceding model-visible message: 6s")
            && first.contains("unavailable")
    );
    let later = render_reading(
        10_000,
        2,
        3,
        None,
        chrono_tz::UTC,
        &RequestZone::Resolved("UTC".into()),
    );
    assert!(
        later.contains("preceding step context: unavailable")
            && later.contains("Interpret otherwise-unqualified")
    );
}

#[test]
fn a_reading_is_due_at_first_then_only_after_the_interval() {
    let clock = Clock::with_zone(chrono_tz::UTC, 600_000);
    let mut history = vec![typed("hi", 1_000)];
    let first = clock
        .reading(&history, 5_000, 1, 1, &[])
        .unwrap()
        .expect("first reading");
    assert!(matches!(first.context, Some(ContextSource::TimeContext)) && first.at == Some(5_000));
    assert!(first.text.contains("preceding model-visible message: 4s"));
    history.push(first);
    assert!(
        clock
            .reading(&history, 5_000 + 599_999, 1, 2, &[])
            .unwrap()
            .is_none(),
        "too soon"
    );
    assert!(
        clock
            .reading(&history, 5_000 + 600_000, 1, 2, &[])
            .unwrap()
            .is_some()
    );
    assert!(
        Clock::with_zone(chrono_tz::UTC, 0)
            .reading(&history, 5_001, 1, 2, &[])
            .unwrap()
            .is_some(),
        "interval zero: every step"
    );
}

#[test]
fn a_bad_reported_zone_is_an_error_never_a_silent_fallback() {
    let clock = Clock::with_zone(chrono_tz::UTC, 0);
    let history = vec![typed("hi", 1)];
    assert!(
        clock
            .reading(&history, 2, 1, 1, &["Not/AZone".to_owned()])
            .is_err()
    );
    let ok = clock
        .reading(&history, 2, 1, 1, &["Asia/Tokyo".to_owned()])
        .unwrap()
        .unwrap();
    assert!(ok.text.contains("+09:00[Asia/Tokyo]"));
}

#[test]
fn the_turn_counts_typed_messages_not_what_the_app_injected() {
    let history = vec![
        typed("a", 1),
        Message::context("c", ContextSource::TimeContext),
        typed("b", 2),
    ];
    assert_eq!(turn_of(&history), 2);
    assert_eq!(turn_of(&[]), 1);
}

fn here() -> Location {
    Location {
        session_name: "main".into(),
        window_index: "1".into(),
        window_name: "code".into(),
        pane_index: "0".into(),
        pane_id: "%3".into(),
        window_active: "1".into(),
        pane_active: "1".into(),
        window_layout: "layout".into(),
    }
}

#[test]
fn a_location_is_said_once_and_again_only_when_it_changes() {
    let mut history = vec![typed("hi", 1)];
    let first = tmux::reading(&history, &here(), 1, 100, 0).expect("first");
    assert!(first.text.starts_with("tmux location (turn 1):") && first.text.contains("main"));
    history.push(first);
    assert!(
        tmux::reading(&history, &here(), 2, 200, 0).is_none(),
        "unchanged"
    );
    let moved = Location {
        window_name: "logs".into(),
        ..here()
    };
    let again = tmux::reading(&history, &moved, 2, 300, 0).expect("changed");
    assert!(again.text.contains("logs"));
    history.push(again);
    assert!(
        tmux::reading(&history, &moved, 3, 400, 1_000).is_none(),
        "inside the refresh interval"
    );
}

struct Fixed(Option<String>);
impl Runner for Fixed {
    fn run(&self, program: &str, args: &[&str]) -> Option<String> {
        assert_eq!(program, "sh");
        assert_eq!(args[0], "-c");
        self.0.clone()
    }
}

#[test]
fn the_pane_query_parses_what_tmux_printed_and_shrugs_off_anything_else() {
    let line = "main\\t1\\tcode\\t0\\t%3\\t1\\t1\\tlayout";
    assert_eq!(tmux::query(&Fixed(Some(line.into())), 7), Some(here()));
    assert!(tmux::query(&Fixed(Some(String::new())), 7).is_none());
    assert!(tmux::query(&Fixed(Some("only\\tthree\\tfields".into())), 7).is_none());
    assert!(tmux::query(&Fixed(None), 7).is_none());
    assert!(tmux::query_script(42).contains("42"));
}
