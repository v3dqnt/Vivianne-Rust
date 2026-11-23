//! Times as the Git tab says them ("5m ago", "Nov 5", "1m 20s"), read from the ISO 8601 text git and GitHub give.

/// Days since 1970-01-01 of a calendar date (proleptic Gregorian).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Seconds since the epoch of `2026-11-20T10:00:00Z` or `2026-11-20T15:30:00+05:30`.
#[must_use]
pub fn epoch(iso: &str) -> Option<i64> {
    let (date, time) = iso.split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, mo, da) = (d.next()??, d.next()??, d.next()??);
    let clock_end = time.find(['Z', '+', '-']).unwrap_or(time.len());
    let mut t = time[..clock_end].split(':');
    let h: i64 = t.next()?.parse().ok()?;
    let mi: i64 = t.next()?.parse().ok()?;
    let s: i64 = t
        .next()
        .and_then(|s| s.split('.').next()?.parse().ok())
        .unwrap_or(0);
    let zone = &time[clock_end..];
    let offset = match zone.chars().next() {
        Some(sign @ ('+' | '-')) => {
            let mut z = zone[1..].split(':');
            let (zh, zm): (i64, i64) = (
                z.next()?.parse().ok()?,
                z.next().and_then(|m| m.parse().ok()).unwrap_or(0),
            );
            (zh * 3600 + zm * 60) * if sign == '-' { -1 } else { 1 }
        }
        _ => 0,
    };
    Some(days_from_civil(y, mo, da) * 86_400 + h * 3600 + mi * 60 + s - offset)
}

#[must_use]
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `Nov 5` or, with the year, `Nov 5, 2026`, from the date as written.
fn calendar(iso: &str, year: bool) -> String {
    let mut d = iso.split('T').next().unwrap_or("").split('-');
    let (y, m, day) = (
        d.next().unwrap_or(""),
        d.next().and_then(|m| m.parse::<usize>().ok()),
        d.next().and_then(|d| d.parse::<u32>().ok()),
    );
    match (m.and_then(|m| MONTHS.get(m.wrapping_sub(1))), day) {
        (Some(month), Some(day)) if year => format!("{month} {day}, {y}"),
        (Some(month), Some(day)) => format!("{month} {day}"),
        _ => iso.to_owned(),
    }
}

/// "just now", "5m ago", "3h ago", "12d ago", then the date.
#[must_use]
pub fn ago(iso: &str, now: i64) -> String {
    let Some(then) = epoch(iso) else {
        return String::new();
    };
    let s = now - then;
    match s {
        ..60 => "just now".to_owned(),
        60..3600 => format!("{}m ago", s / 60),
        3600..86_400 => format!("{}h ago", s / 3600),
        86_400..2_592_000 => format!("{}d ago", s / 86_400),
        _ => calendar(iso, true),
    }
}

/// A commit's age in the history list: "3h ago" for the week, then "Nov 5".
#[must_use]
pub fn when(iso: &str, now: i64) -> String {
    match epoch(iso) {
        Some(then) if now - then < 7 * 86_400 => ago(iso, now),
        Some(_) => calendar(iso, false),
        None => iso.to_owned(),
    }
}

/// "45s", "3m 20s", "1h 5m" between two times; to `now` when `to` is empty (still running).
#[must_use]
pub fn duration(from: Option<&str>, to: Option<&str>, now: i64) -> String {
    let Some(start) = from.and_then(epoch) else {
        return String::new();
    };
    let end = to.and_then(epoch).unwrap_or(now);
    let s = (end - start).max(0);
    match s {
        ..60 => format!("{s}s"),
        60..3600 => format!("{}m {}s", s / 60, s % 60),
        _ => format!("{}h {}m", s / 3600, (s % 3600) / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_times_become_epoch_seconds() {
        assert_eq!(epoch("1970-01-02T00:00:00Z"), Some(86_400));
        assert_eq!(epoch("2026-11-20T10:00:00Z"), Some(1_795_168_800));
        // 15:30 at +05:30 is 10:00 UTC.
        assert_eq!(
            epoch("2026-11-20T15:30:00+05:30"),
            epoch("2026-11-20T10:00:00Z")
        );
        assert_eq!(epoch("not a time"), None);
    }

    #[test]
    fn ages_read_the_way_the_old_view_said_them() {
        let then = "2026-11-20T10:00:00Z";
        let at = |secs: i64| epoch(then).map_or(0, |t| t + secs);
        assert_eq!(ago(then, at(10)), "just now");
        assert_eq!(ago(then, at(300)), "5m ago");
        assert_eq!(ago(then, at(7200)), "2h ago");
        assert_eq!(ago(then, at(3 * 86_400)), "3d ago");
        assert_eq!(ago(then, at(60 * 86_400)), "Nov 20, 2026");
        assert_eq!(when(then, at(10 * 86_400)), "Nov 20");
    }

    #[test]
    fn durations_are_short() {
        let (a, b) = ("2026-11-20T10:00:00Z", "2026-11-20T10:03:20Z");
        assert_eq!(duration(Some(a), Some(b), 0), "3m 20s");
        assert_eq!(duration(Some(a), Some("2026-11-20T10:00:45Z"), 0), "45s");
        assert_eq!(duration(None, None, 0), "");
    }
}
