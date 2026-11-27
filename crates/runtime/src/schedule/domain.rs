//! A scheduled task and when it fires. Pure: the clock is always passed in, nothing is read or written.
//!
//! A schedule is one of: `after` (a delay), `at` (an instant, or a local date and time in a zone), `every` (a fixed
//! interval), `daily`, `weekly` (a local time in a named zone) or `cron` (five fields, in a named zone). Daily, weekly
//! and cron follow the zone's calendar: a local time that does not exist that day (the clocks went forward) is skipped
//! for that day, and one that happens twice (the clocks went back) fires the first time. After being away, a recurring
//! task is delivered once, for its latest missed occurrence, not once per miss.

use std::str::FromStr;

use chrono::{DateTime, Datelike, Duration, LocalResult, NaiveDate, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use cron::Schedule;
use serde::{Deserialize, Serialize};

/// The shortest interval `every` and a cron schedule may fire at.
pub const MIN_EVERY_SECONDS: u64 = 60;
pub const MAX_TITLE: usize = 120;
/// Instants before year 0000 or after 9999 cannot be written as four-digit RFC 3339 and are refused.
const MIN_MS: i64 = -62_167_219_200_000;
const MAX_MS: i64 = 253_402_300_799_999;
/// Days looked ahead (or back) for a date that fits a rule before giving up.
const SEARCH_DAYS: i64 = 3_700;
/// Cron occurrences stepped through when catching up before jumping close to now.
const CRON_STEPS: usize = 10_000;

/// Why an input was refused. The code is stable and what a tool answers with.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScheduleError {
    #[error("{0}")]
    InvalidPrompt(String),
    #[error("{0}")]
    InvalidSelector(String),
    #[error("{0}")]
    InvalidRule(String),
    #[error("{0}")]
    InvalidTimeZone(String),
    #[error("The scheduled time must be strictly in the future.")]
    NotFuture,
    #[error("{0}")]
    TimeOutOfRange(String),
    #[error("every_seconds must be at least {MIN_EVERY_SECONDS}.")]
    FrequencyTooHigh,
}

impl ScheduleError {
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidPrompt(_) => "invalid_prompt",
            Self::InvalidSelector(_) => "invalid_selector",
            Self::InvalidRule(_) => "invalid_rule",
            Self::InvalidTimeZone(_) => "invalid_time_zone",
            Self::NotFuture => "not_future",
            Self::TimeOutOfRange(_) => "time_out_of_range",
            Self::FrequencyTooHigh => "frequency_too_high",
        }
    }
}

/// When a task fires, as written (`time` is `HH:MM:SS` or `HH:MM:SS.mmm`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Timing {
    After { after_seconds: u64 },
    /// An RFC 3339 instant with its offset.
    At { at: String },
    /// A local date and time in a named zone.
    AtLocal { date: String, time: String, time_zone: String },
    Every { every_seconds: u64 },
    Daily { time: String, time_zone: String },
    Weekly { time: String, time_zone: String, weekdays: Vec<u8> },
    Cron { expression: String, time_zone: String },
}

/// Where a fired task is delivered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Target {
    /// An existing conversation.
    Session { session_id: String },
    /// A new conversation, made once and then reused.
    NewOnce { session_id: String, create: CreateSpec },
    /// A new conversation for every run.
    New { create: CreateSpec },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

/// A task. `scheduled_at` (milliseconds since 1970) is the next time it is due; for a recurring task, the earliest one
/// not yet delivered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub prompt: String,
    pub timing: Timing,
    pub scheduled_at: i64,
    pub target: Target,
    pub active: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

/// One delivery: the occurrence that fired, and when the task is next due (none: it has ended).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Occurrence {
    pub occurrence_at: i64,
    pub next_scheduled_at: Option<i64>,
}

#[must_use]
pub fn is_recurring(timing: &Timing) -> bool {
    !matches!(timing, Timing::After { .. } | Timing::At { .. } | Timing::AtLocal { .. })
}

pub fn check_title(title: &str) -> Result<String, ScheduleError> {
    let title = title.trim();
    if title.is_empty() {
        return Err(ScheduleError::InvalidPrompt("title is required and must be non-empty after trimming.".into()));
    }
    if title.chars().count() > MAX_TITLE {
        return Err(ScheduleError::InvalidPrompt(format!("title must be at most {MAX_TITLE} characters.")));
    }
    if title.chars().any(char::is_control) {
        return Err(ScheduleError::InvalidPrompt("title must not contain control characters.".into()));
    }
    Ok(title.to_owned())
}

pub fn check_prompt(prompt: &str) -> Result<String, ScheduleError> {
    if prompt.trim().is_empty() {
        return Err(ScheduleError::InvalidPrompt("prompt must be non-empty after trimming.".into()));
    }
    Ok(prompt.to_owned())
}

fn zone(name: &str) -> Result<Tz, ScheduleError> {
    Tz::from_str(name.trim()).map_err(|_| {
        ScheduleError::InvalidTimeZone(format!("`{name}` is not a time zone name such as Europe/Paris or UTC."))
    })
}

fn clock(value: &str, what: &str) -> Result<NaiveTime, ScheduleError> {
    let bytes = value.as_bytes();
    let shaped = value.len() >= 8
        && bytes[2] == b':'
        && bytes[5] == b':'
        && (value.len() == 8 || (bytes[8] == b'.' && (10..=12).contains(&value.len())))
        && value.char_indices().all(|(i, c)| matches!(i, 2 | 5 | 8) && matches!(c, ':' | '.') || c.is_ascii_digit());
    NaiveTime::parse_from_str(value, "%H:%M:%S%.f").ok().filter(|_| shaped).ok_or_else(|| {
        ScheduleError::InvalidRule(format!("{what} must be a local time HH:MM:SS (optionally with .mmm), like 09:30:00."))
    })
}

fn weekdays(list: &[u8]) -> Result<Vec<u8>, ScheduleError> {
    if list.is_empty() {
        return Err(ScheduleError::InvalidRule("weekly.weekdays must be a non-empty array of ISO weekday numbers.".into()));
    }
    let mut seen = std::collections::BTreeSet::new();
    for day in list {
        if !(1..=7).contains(day) {
            return Err(ScheduleError::InvalidRule(
                "Each weekly.weekdays entry must be an integer from 1 (Monday) through 7 (Sunday).".into(),
            ));
        }
        if !seen.insert(*day) {
            return Err(ScheduleError::InvalidRule(format!("weekly.weekdays must not repeat weekday {day}.")));
        }
    }
    Ok(seen.into_iter().collect())
}

/// The day-of-week field of a standard cron expression (0 or 7 is Sunday, 1 Monday) as the `cron` crate reads it
/// (1 is Sunday, 2 Monday ... 7 Saturday): numbers, ranges and steps are written out one by one; names are left alone.
fn crate_weekdays(field: &str) -> Result<String, ScheduleError> {
    let bad = || ScheduleError::InvalidRule(format!("`{field}` is not a day-of-week field."));
    let number = |text: &str| -> Result<u32, ScheduleError> {
        let n: u32 = text.parse().map_err(|_| bad())?;
        if n > 7 { Err(bad()) } else { Ok(n) }
    };
    let mapped = |n: u32| n % 7 + 1;
    let mut parts = Vec::new();
    for item in field.split(',') {
        let (range, step) = match item.split_once('/') {
            Some((range, step)) => (range, Some(step.parse::<u32>().ok().filter(|s| *s > 0).ok_or_else(bad)?)),
            None => (item, None),
        };
        if range == "*" || !range.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            // `*`, `*/2` and names mean the same in both.
            parts.push(item.to_owned());
            continue;
        }
        let (from, to) = match range.split_once('-') {
            Some((from, to)) => (number(from)?, number(to)?),
            None if step.is_some() => (number(range)?, 6),
            None => (number(range)?, number(range)?),
        };
        // `5-7` is Friday to Sunday; Sunday is both 0 and 7.
        let to = if to < from { return Err(bad()) } else { to };
        parts.extend((from..=to).step_by(step.unwrap_or(1) as usize).map(|n| mapped(n).to_string()));
    }
    Ok(parts.join(","))
}

/// A standard five-field cron expression as a schedule.
pub fn cron_schedule(expression: &str) -> Result<Schedule, ScheduleError> {
    let fields: Vec<&str> = expression.split_whitespace().collect();
    if fields.len() != 5 {
        return Err(ScheduleError::InvalidRule(format!(
            "cron takes five fields (minute hour day-of-month month day-of-week), not {}.",
            fields.len()
        )));
    }
    let weekdays = crate_weekdays(fields[4])?;
    let written = format!("0 {} {} {} {} {}", fields[0], fields[1], fields[2], fields[3], weekdays);
    Schedule::from_str(&written).map_err(|e| ScheduleError::InvalidRule(format!("`{expression}` is not a cron expression: {e}")))
}

/// Whether `ms` can be written as a four-digit-year instant.
#[must_use]
pub fn representable(ms: i64) -> bool {
    (MIN_MS..=MAX_MS).contains(&ms)
}

fn ms_of<T: TimeZone>(t: &DateTime<T>) -> i64 {
    t.timestamp_millis()
}

/// The instant a local date and time is, in `tz`: the first when it happens twice, none when it does not exist.
fn local_instant(date: NaiveDate, time: NaiveTime, tz: Tz) -> Option<i64> {
    match tz.from_local_datetime(&date.and_time(time)) {
        LocalResult::Single(t) | LocalResult::Ambiguous(t, _) => Some(ms_of(&t)),
        LocalResult::None => None,
    }
}

fn local_date(ms: i64, tz: Tz) -> Option<NaiveDate> {
    Some(DateTime::from_timestamp_millis(ms)?.with_timezone(&tz).date_naive())
}

/// The first occurrence of a daily/weekly rule strictly after `now` (and on a later local date than `after_date`).
fn next_matching(
    time: NaiveTime,
    tz: Tz,
    now: i64,
    days: Option<&[u8]>,
    after_date: Option<NaiveDate>,
) -> Option<i64> {
    let mut date = local_date(now, tz)?;
    if let Some(after) = after_date
        && date <= after
    {
        date = after.succ_opt()?;
    }
    for _ in 0..SEARCH_DAYS {
        let wanted = days.is_none_or(|d| d.contains(&u8::try_from(date.weekday().number_from_monday()).unwrap_or(0)));
        if wanted
            && let Some(at) = local_instant(date, time, tz)
            && at > now
            && representable(at)
        {
            return Some(at);
        }
        date = date.succ_opt()?;
    }
    None
}

/// The latest occurrence at or before `accepted_at`, never earlier than the committed `saved` target.
fn latest_due(time: NaiveTime, tz: Tz, days: Option<&[u8]>, accepted_at: i64, saved: i64) -> i64 {
    // One day past the decision's UTC date: zone offsets are under a day, so no due date is missed.
    let Some(mut date) = DateTime::from_timestamp_millis(accepted_at).map(|t| t.date_naive() + Duration::days(1)) else {
        return saved;
    };
    for _ in 0..SEARCH_DAYS {
        let wanted = days.is_none_or(|d| d.contains(&u8::try_from(date.weekday().number_from_monday()).unwrap_or(0)));
        if wanted
            && let Some(at) = local_instant(date, time, tz)
            && at <= accepted_at
        {
            return at.max(saved);
        }
        let Some(previous) = date.pred_opt() else { break };
        date = previous;
    }
    saved
}

fn cron_after(schedule: &Schedule, tz: Tz, ms: i64) -> Option<i64> {
    let from = DateTime::from_timestamp_millis(ms)?.with_timezone(&tz);
    schedule.after(&from).next().map(|t| ms_of(&t)).filter(|t| representable(*t))
}

/// The first time `timing` fires after `now`.
fn first_target(timing: &Timing, now: i64) -> Result<i64, ScheduleError> {
    let out_of_range = |what: &str| ScheduleError::TimeOutOfRange(format!("No future {what} occurrence is representable as a four-digit-year UTC instant."));
    let target = match timing {
        Timing::After { after_seconds } => now.saturating_add(i64::try_from(*after_seconds).unwrap_or(i64::MAX).saturating_mul(1000)),
        Timing::At { at } => DateTime::parse_from_rfc3339(at)
            .map_err(|_| ScheduleError::InvalidSelector("at must be an RFC 3339 instant with an offset, like 2026-12-01T09:00:00+01:00, or a local date, time and time_zone.".into()))?
            .timestamp_millis(),
        Timing::AtLocal { date, time, time_zone } => {
            let date = NaiveDate::parse_from_str(date, "%Y-%m-%d")
                .map_err(|_| ScheduleError::InvalidSelector("at.date must be YYYY-MM-DD.".into()))?;
            local_instant(date, clock(time, "at.time")?, zone(time_zone)?)
                .ok_or_else(|| ScheduleError::InvalidRule("The local at time does not exist in the selected time zone.".into()))?
        }
        Timing::Every { every_seconds } => now.saturating_add(i64::try_from(*every_seconds).unwrap_or(i64::MAX).saturating_mul(1000)),
        Timing::Daily { time, time_zone } => next_matching(clock(time, "daily.time")?, zone(time_zone)?, now, None, None).ok_or_else(|| out_of_range("daily"))?,
        Timing::Weekly { time, time_zone, weekdays: days } => {
            next_matching(clock(time, "weekly.time")?, zone(time_zone)?, now, Some(days), None).ok_or_else(|| out_of_range("weekly"))?
        }
        Timing::Cron { expression, time_zone } => {
            cron_after(&cron_schedule(expression)?, zone(time_zone)?, now).ok_or_else(|| out_of_range("cron"))?
        }
    };
    if !representable(target) {
        return Err(ScheduleError::TimeOutOfRange("The scheduled time must be representable as a four-digit-year RFC 3339 UTC instant.".into()));
    }
    if target <= now {
        return Err(ScheduleError::NotFuture);
    }
    Ok(target)
}

/// Checks a timing and writes it in its canonical form (trimmed zone, ordered weekdays).
pub fn normalize(timing: &Timing) -> Result<Timing, ScheduleError> {
    Ok(match timing {
        Timing::After { after_seconds } => {
            if *after_seconds == 0 {
                return Err(ScheduleError::InvalidRule("after_seconds must be a positive whole number.".into()));
            }
            timing.clone()
        }
        Timing::At { .. } => timing.clone(),
        Timing::AtLocal { date, time, time_zone } => Timing::AtLocal { date: date.clone(), time: time.clone(), time_zone: zone(time_zone)?.name().to_owned() },
        Timing::Every { every_seconds } => {
            if *every_seconds < MIN_EVERY_SECONDS {
                return Err(ScheduleError::FrequencyTooHigh);
            }
            timing.clone()
        }
        Timing::Daily { time, time_zone } => {
            clock(time, "daily.time")?;
            Timing::Daily { time: time.clone(), time_zone: zone(time_zone)?.name().to_owned() }
        }
        Timing::Weekly { time, time_zone, weekdays: days } => {
            clock(time, "weekly.time")?;
            Timing::Weekly { time: time.clone(), time_zone: zone(time_zone)?.name().to_owned(), weekdays: weekdays(days)? }
        }
        Timing::Cron { expression, time_zone } => {
            cron_schedule(expression)?;
            Timing::Cron { expression: expression.split_whitespace().collect::<Vec<_>>().join(" "), time_zone: zone(time_zone)?.name().to_owned() }
        }
    })
}

/// A new task, due at the first occurrence after `now`. Nothing is made if any part is refused.
pub fn create(
    id: &str,
    title: &str,
    prompt: &str,
    timing: &Timing,
    target: Target,
    now: i64,
) -> Result<Task, ScheduleError> {
    let title = check_title(title)?;
    let prompt = check_prompt(prompt)?;
    let timing = normalize(timing)?;
    let scheduled_at = first_target(&timing, now)?;
    Ok(Task {
        id: id.to_owned(),
        title,
        prompt,
        timing,
        scheduled_at,
        target,
        active: true,
        created_at: now,
        updated_at: now,
    })
}

/// A task with its timing changed: due at the first occurrence after `now`.
pub fn retime(task: &Task, timing: &Timing, now: i64) -> Result<Task, ScheduleError> {
    let timing = normalize(timing)?;
    let scheduled_at = first_target(&timing, now)?;
    Ok(Task { timing, scheduled_at, updated_at: now, ..task.clone() })
}

/// What a recurring task delivers when decided at `accepted_at` (at or after its `scheduled_at`): the latest due
/// occurrence, and the next one strictly in the future. `None` for a one-shot or when `accepted_at` is early.
#[must_use]
pub fn resolve(task: &Task, accepted_at: i64) -> Option<Occurrence> {
    if accepted_at < task.scheduled_at || !representable(accepted_at) {
        return None;
    }
    match &task.timing {
        Timing::After { .. } | Timing::At { .. } | Timing::AtLocal { .. } => None,
        Timing::Every { every_seconds } => {
            let interval = i64::try_from(*every_seconds).ok()?.checked_mul(1000)?;
            let steps = (accepted_at - task.scheduled_at) / interval;
            let occurrence_at = task.scheduled_at + steps * interval;
            let next = occurrence_at + interval;
            Some(Occurrence { occurrence_at, next_scheduled_at: representable(next).then_some(next) })
        }
        Timing::Daily { time, time_zone } => resolve_calendar(time, time_zone, None, task, accepted_at),
        Timing::Weekly { time, time_zone, weekdays: days } => resolve_calendar(time, time_zone, Some(days), task, accepted_at),
        Timing::Cron { expression, time_zone } => {
            let schedule = cron_schedule(expression).ok()?;
            let tz = zone(time_zone).ok()?;
            let mut latest = task.scheduled_at;
            let mut at = task.scheduled_at;
            for _ in 0..CRON_STEPS {
                match cron_after(&schedule, tz, at) {
                    Some(next) if next <= accepted_at => {
                        latest = next;
                        at = next;
                    }
                    _ => break,
                }
            }
            // A schedule left running for years: start again close to the decision rather than step through it all.
            if latest < accepted_at - 400 * 86_400_000 {
                let mut at = accepted_at - 400 * 86_400_000;
                while let Some(next) = cron_after(&schedule, tz, at).filter(|n| *n <= accepted_at) {
                    latest = next;
                    at = next;
                }
            }
            Some(Occurrence { occurrence_at: latest, next_scheduled_at: cron_after(&schedule, tz, accepted_at) })
        }
    }
}

fn resolve_calendar(time: &str, tz_name: &str, days: Option<&Vec<u8>>, task: &Task, accepted_at: i64) -> Option<Occurrence> {
    let (time, tz) = (clock(time, "time").ok()?, zone(tz_name).ok()?);
    let days = days.map(Vec::as_slice);
    let occurrence_at = latest_due(time, tz, days, accepted_at, task.scheduled_at);
    let next_scheduled_at = next_matching(time, tz, accepted_at, days, local_date(occurrence_at, tz));
    Some(Occurrence { occurrence_at, next_scheduled_at })
}

/// How a due task reads to the model.
const FRAMING: &str = "This is a scheduled message from the user";

/// The message a due task becomes. Its dynamic parts are JSON-escaped, so a prompt cannot pass for the lines around it.
#[must_use]
pub fn reminder(id: &str, prompt: &str, occurrence: &str) -> String {
    format!(
        "[SCHEDULE REMINDER]\n{FRAMING}\nschedule_id_json: {}\noccurrence_at: {occurrence}\nreminder_prompt_json: {}",
        serde_json::Value::from(id),
        serde_json::Value::from(prompt)
    )
}

/// Several tasks due together, one line of JSON in the order given.
#[must_use]
pub fn reminder_batch(due: &[(&str, &str, &str)]) -> String {
    let payload: Vec<serde_json::Value> = due
        .iter()
        .map(|(id, prompt, at)| serde_json::json!({ "schedule_id": id, "occurrence_at": at, "reminder_prompt": prompt }))
        .collect();
    format!("[SCHEDULE REMINDER BATCH]\n{FRAMING}\nreminders_json: {}", serde_json::Value::Array(payload))
}

/// A canonical UTC instant: `2026-03-08T07:30:00.000Z`.
#[must_use]
pub fn iso(ms: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(ms).map_or_else(String::new, |t| t.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
}

#[cfg(test)]
#[path = "domain_tests.rs"]
mod tests;
