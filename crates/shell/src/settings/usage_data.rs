//! What Settings, Account shows, read from the usage the app keeps (`vivianne_ai::inference::usage_store`) and
//! turned into the page's own types. The page itself draws from `Values`; this is the only place its numbers
//! come from.

use std::collections::BTreeMap;

use gpui_kit::App;
use vivianne_ai::inference::usage_store::{self, DayUsage};

use super::values::{ModelUsage, Shared, UsageDay};
use crate::backend;

/// The range the by-model table starts on.
pub const DEFAULT_RANGE: &str = "30";

/// Whole days since 1970-01-01 for a `YYYY-MM-DD` date (Howard Hinnant's `days_from_civil`), or `None` if it
/// is not one.
pub fn epoch_day(date: &str) -> Option<i64> {
    let mut parts = date.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(era * 146_097 + day_of_era - 719_468)
}

/// Today as a whole epoch day, by the local calendar, because that is how usage days are filed.
pub fn today() -> i64 {
    epoch_day(&usage_store::today_key()).unwrap_or(0)
}

/// The cost to show for usage that may include unpriced requests: `None` when none of it could be priced, so
/// it reads as unknown, never as $0.
fn shown_cost(usage: &DayUsage) -> Option<f64> {
    (!usage.has_unknown_cost || usage.cost > 0.0).then_some(usage.cost)
}

/// Each day's usage as the page's days counted back from `today` (a whole epoch day). Days in the future
/// (a clock that moved) and keys that are not dates are left out.
pub fn usage_days(history: &BTreeMap<String, DayUsage>, today: i64) -> Vec<UsageDay> {
    history
        .iter()
        .filter_map(|(date, day)| {
            let offset = u32::try_from(today - epoch_day(date)?).ok()?;
            Some(UsageDay {
                offset,
                requests: day.requests,
                input: day.input,
                output: day.output,
                cost: shown_cost(day),
                unpriced: day.has_unknown_cost,
            })
        })
        .collect()
}

/// The per-model totals (keyed `"<provider>/<model id>"`) as the page's rows, busiest first.
pub fn model_usage(totals: &BTreeMap<String, DayUsage>) -> Vec<ModelUsage> {
    let mut rows: Vec<ModelUsage> = totals
        .iter()
        .map(|(key, usage)| {
            let (provider, model) = key.split_once('/').unwrap_or(("", key));
            ModelUsage {
                model: model.to_owned(),
                provider: provider.to_owned(),
                requests: usage.requests,
                input: usage.input,
                output: usage.output,
                cost: shown_cost(usage),
                unpriced: usage.has_unknown_cost,
            }
        })
        .collect();
    rows.sort_by(|a, b| (b.input + b.output).cmp(&(a.input + a.output)).then_with(|| a.model.cmp(&b.model)));
    rows
}

/// The first day a range covers (`"7"` and `"30"` are that many days counting today), or `None` for all time.
pub fn since_for(range: &str) -> Option<String> {
    let days: i64 = range.parse().ok()?;
    Some(usage_store::days_ago_key(days.max(1) - 1))
}

/// Reads the history and the per-model totals the first time the page is drawn.
pub fn load(values: &Shared, cx: &mut App) {
    if backend::get().is_none() {
        let mut v = values.borrow_mut();
        v.usage_loaded = true;
        v.usage_loading = false;
        return;
    }
    let since = since_for(&values.borrow().usage_range);
    let values = values.clone();
    backend::run(
        cx,
        async move { (usage_store::read_history().await, usage_store::read_usage_by_model(since.as_deref()).await) },
        move |(history, by_model), cx| {
            {
                let mut v = values.borrow_mut();
                v.usage_days = usage_days(&history, today());
                v.model_usage = model_usage(&by_model);
                v.usage_loaded = true;
                v.usage_loading = false;
            }
            cx.refresh_windows();
        },
    );
}

/// Reads the per-model totals again, for the range the picker now shows.
pub fn load_models(values: &Shared, cx: &mut App) {
    let since = since_for(&values.borrow().usage_range);
    let values = values.clone();
    let asked = values.borrow().usage_range.clone();
    backend::run(
        cx,
        async move { usage_store::read_usage_by_model(since.as_deref()).await },
        move |by_model, cx| {
            {
                let mut v = values.borrow_mut();
                // A slower answer for a range that is no longer the one shown must not replace the newer one.
                if v.usage_range == asked {
                    v.model_usage = model_usage(&by_model);
                }
            }
            cx.refresh_windows();
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(input: u64, cost: f64, unknown: bool) -> DayUsage {
        DayUsage { input, output: 1, cache_read: 0, cache_write: 0, cost, requests: 2, has_unknown_cost: unknown }
    }

    #[test]
    fn a_date_becomes_its_epoch_day_and_junk_does_not() {
        assert_eq!(epoch_day("1970-01-01"), Some(0));
        assert_eq!(epoch_day("1970-01-02"), Some(1));
        assert_eq!(epoch_day("2000-03-01"), Some(11_017));
        assert_eq!(epoch_day("2026-11-20"), Some(20_777));
        assert_eq!(epoch_day("2026-13-01"), None);
        assert_eq!(epoch_day("yesterday"), None);
        assert_eq!(epoch_day("2026-11-20-1"), None);
    }

    #[test]
    fn days_are_counted_back_from_today_and_the_future_is_left_out() {
        let today = epoch_day("2026-11-20").unwrap();
        let history = BTreeMap::from([
            ("2026-11-20".to_owned(), day(100, 0.5, false)),
            ("2026-11-18".to_owned(), day(50, 0.0, true)),
            ("2026-11-25".to_owned(), day(9, 0.0, false)),
            ("not a date".to_owned(), day(9, 0.0, false)),
        ]);
        let days = usage_days(&history, today);
        assert_eq!(days.len(), 2);
        let by_offset = |n: u32| days.iter().find(|d| d.offset == n);
        assert_eq!(by_offset(0).map(|d| (d.input, d.cost, d.unpriced)), Some((100, Some(0.5), false)));
        assert_eq!(by_offset(2).map(|d| (d.cost, d.unpriced)), Some((None, true)), "a day with only unpriced calls has no cost, not $0");
    }

    #[test]
    fn models_split_at_the_first_slash_and_the_busiest_comes_first() {
        let totals = BTreeMap::from([
            ("nebius/nvidia/nemotron".to_owned(), day(10, 0.1, false)),
            ("harness/claude-code".to_owned(), day(500, 0.0, true)),
        ]);
        let rows = model_usage(&totals);
        assert_eq!((rows[0].provider.as_str(), rows[0].model.as_str(), rows[0].cost), ("harness", "claude-code", None));
        assert_eq!((rows[1].provider.as_str(), rows[1].model.as_str()), ("nebius", "nvidia/nemotron"));
    }

    #[test]
    fn a_partly_priced_day_keeps_its_known_cost_and_says_it_is_partial() {
        let rows = model_usage(&BTreeMap::from([("p/m".to_owned(), day(10, 0.25, true))]));
        assert_eq!((rows[0].cost, rows[0].unpriced), (Some(0.25), true));
    }

    #[test]
    fn a_range_covers_that_many_days_counting_today() {
        assert_eq!(since_for("all"), None);
        assert_eq!(since_for("7"), Some(usage_store::days_ago_key(6)));
        assert_eq!(since_for("1"), Some(usage_store::today_key()));
    }
}
