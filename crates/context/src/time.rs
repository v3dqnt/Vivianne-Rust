//! The clock. On eligible steps the model gets a reading: the time with its offset and IANA zone, which zone applies to
//! unqualified dates in the request, and how long since the previous model-visible message. Port of `time-context`.
//!
//! The reading is one durable user-role message. Whether one is due, what it says, and how a zone is chosen are pure
//! functions of the conversation and a clock value, so they are tested without a clock.

use chrono::{DateTime, TimeZone as _, Utc};
use chrono_tz::Tz;
use vivianne_core::Message;
use vivianne_core::context::ContextSource;

/// A reading is not repeated sooner than this unless the interval is set to zero.
pub const DEFAULT_REFRESH_MS: u64 = 600_000;

/// How the zone of a request was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestZone {
    /// Exactly one valid zone was reported for the request.
    Resolved(String),
    /// More than one was.
    Mixed(Vec<String>),
    /// None was.
    Missing,
}

/// Why a reported zone was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZoneError {
    Blank,
    Padded,
    Unsupported,
    /// A name that is not the canonical spelling (a case or alias variant).
    NotCanonical,
}

/// A zone name the app accepts: canonical `UTC` or IANA `Area/Location`, exactly as the zone database spells it.
pub fn canonical_zone(value: &str) -> Result<Tz, ZoneError> {
    if value.is_empty() {
        return Err(ZoneError::Blank);
    }
    if value.trim() != value {
        return Err(ZoneError::Padded);
    }
    let tz: Tz = value.parse().map_err(|_| ZoneError::Unsupported)?;
    if tz.name() != value {
        return Err(ZoneError::NotCanonical);
    }
    Ok(tz)
}

/// The zone of one request from the zones reported for it, sorted and without repeats.
pub fn derive_request_zone(reported: &[String]) -> Result<RequestZone, ZoneError> {
    let mut zones: Vec<String> = Vec::new();
    for zone in reported {
        canonical_zone(zone)?;
        if !zones.contains(zone) {
            zones.push(zone.clone());
        }
    }
    zones.sort();
    Ok(match zones.len() {
        0 => RequestZone::Missing,
        1 => RequestZone::Resolved(zones.remove(0)),
        _ => RequestZone::Mixed(zones),
    })
}

/// The one-line policy the model is given for the request's zone.
#[must_use]
pub fn render_zone_policy(zone: &RequestZone) -> String {
    match zone {
        RequestZone::Resolved(z) => format!("Browser time zone for this request: {z}. Interpret otherwise-unqualified dates and times in this zone."),
        RequestZone::Mixed(zones) => format!(
            "Browser time zone for this request: mixed {}. Ask the user to clarify otherwise-unqualified dates and times.",
            serde_json::to_string(zones).unwrap_or_default()
        ),
        RequestZone::Missing => "Browser time zone for this request: unavailable. Ask the user to clarify otherwise-unqualified dates and times.".to_owned(),
    }
}

/// `2026-11-23T14:05:09+05:30[Asia/Kolkata]`: local fields, numeric offset, zone label.
#[must_use]
pub fn format_timestamp(now_ms: i64, tz: Tz) -> String {
    let at: DateTime<Utc> = Utc
        .timestamp_millis_opt(now_ms)
        .single()
        .unwrap_or_default();
    let local = at.with_timezone(&tz);
    let offset = local.format("%:z");
    format!(
        "{}{offset}[{}]",
        local.format("%Y-%m-%dT%H:%M:%S"),
        tz.name()
    )
}

/// A span as compact whole-second units: `1d 2h 3m 4s`. A clock that ran backward is a property of the clock, not of the
/// span, so it reads as zero.
#[must_use]
pub fn format_duration(elapsed_ms: i64) -> String {
    let total = elapsed_ms.max(0) / 1000;
    let (days, hours, minutes, seconds) = (
        total / 86_400,
        total % 86_400 / 3600,
        total % 3600 / 60,
        total % 60,
    );
    let mut parts = Vec::new();
    if days > 0 {
        parts.push(format!("{days}d"));
    }
    if hours > 0 {
        parts.push(format!("{hours}h"));
    }
    if minutes > 0 {
        parts.push(format!("{minutes}m"));
    }
    parts.push(format!("{seconds}s"));
    parts.join(" ")
}

/// The text of one reading.
#[must_use]
pub fn render_reading(
    now_ms: i64,
    turn: usize,
    step: usize,
    previous_ms: Option<i64>,
    tz: Tz,
    zone: &RequestZone,
) -> String {
    let elapsed =
        previous_ms.map_or_else(|| "unavailable".to_owned(), |p| format_duration(now_ms - p));
    let baseline = if step == 1 {
        "model-visible message"
    } else {
        "step context"
    };
    format!(
        "Time sampled while preparing turn {turn}, step {step}: {}\n{}\nElapsed since the preceding {baseline}: {elapsed}.",
        format_timestamp(now_ms, tz),
        render_zone_policy(zone)
    )
}

/// The zone of the machine this runs on, as an IANA name.
#[must_use]
pub fn process_zone() -> Option<Tz> {
    iana_time_zone::get_timezone().ok()?.parse().ok()
}

/// Settings of the clock context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// The zone used when the request names none. `None`: the machine's.
    pub time_zone: Option<String>,
    /// Minimum milliseconds between readings in one conversation; 0 reads at every eligible step.
    pub refresh_interval_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            time_zone: None,
            refresh_interval_ms: DEFAULT_REFRESH_MS,
        }
    }
}

/// The clock context of one conversation.
#[derive(Debug, Clone)]
pub struct Clock {
    fallback: Tz,
    refresh_interval_ms: u64,
}

impl Clock {
    /// Fails when the configured zone is unknown or the machine's cannot be found, so a bad setting is seen at start.
    pub fn new(config: &Config) -> Result<Self, String> {
        let fallback = match &config.time_zone {
            Some(name) => name
                .parse::<Tz>()
                .map_err(|_| format!("time-context: invalid IANA timeZone {name:?}"))?,
            None => process_zone().ok_or("time-context: failed to resolve the system time zone")?,
        };
        Ok(Self {
            fallback,
            refresh_interval_ms: config.refresh_interval_ms,
        })
    }

    /// A clock with a fixed fallback zone, for tests and callers that already chose one.
    #[must_use]
    pub fn with_zone(fallback: Tz, refresh_interval_ms: u64) -> Self {
        Self {
            fallback,
            refresh_interval_ms,
        }
    }

    /// The reading to add before step `step` of turn `turn`, if one is due. `reported_zones` are the zones the client
    /// reported for the open turn. `None` when it is too soon after the last reading. A reported zone that is not valid
    /// is an error: a request must not silently fall back to another zone.
    pub fn reading(
        &self,
        history: &[Message],
        now_ms: i64,
        turn: usize,
        step: usize,
        reported_zones: &[String],
    ) -> Result<Option<Message>, ZoneError> {
        let last_injection = last_reading_time(history);
        if self.refresh_interval_ms > 0
            && let Some(last) = last_injection
            && now_ms >= last
            && ((now_ms - last).unsigned_abs()) < self.refresh_interval_ms
        {
            return Ok(None);
        }
        let zone = derive_request_zone(reported_zones)?;
        let tz = match &zone {
            RequestZone::Resolved(z) => canonical_zone(z)?,
            _ => self.fallback,
        };
        let previous = if step == 1 {
            last_model_visible_time(history)
        } else {
            last_injection_in_turn(history)
        };
        let text = render_reading(now_ms, turn, step, previous, tz, &zone);
        Ok(Some(
            Message::context(text, ContextSource::TimeContext)
                .at(u64::try_from(now_ms).unwrap_or(0)),
        ))
    }
}

fn millis(m: &Message) -> Option<i64> {
    m.at.and_then(|a| i64::try_from(a).ok())
}

fn is_reading(m: &Message) -> bool {
    matches!(m.context, Some(ContextSource::TimeContext))
}

/// When the newest reading was added.
fn last_reading_time(history: &[Message]) -> Option<i64> {
    history
        .iter()
        .rev()
        .find(|m| is_reading(m))
        .and_then(millis)
}

/// When the newest message the model sees (anything but a reading itself) was added.
fn last_model_visible_time(history: &[Message]) -> Option<i64> {
    history
        .iter()
        .rev()
        .filter(|m| !is_reading(m))
        .find_map(millis)
}

/// When the newest reading of the open turn was added: the readings after the last typed message.
fn last_injection_in_turn(history: &[Message]) -> Option<i64> {
    let from = history
        .iter()
        .rposition(Message::is_typed)
        .map_or(0, |i| i + 1);
    history[from..]
        .iter()
        .rev()
        .find(|m| is_reading(m))
        .and_then(millis)
}

/// The turn number: how many messages the person typed, counting the one being answered.
#[must_use]
pub fn turn_of(history: &[Message]) -> usize {
    history.iter().filter(|m| m.is_typed()).count().max(1)
}
