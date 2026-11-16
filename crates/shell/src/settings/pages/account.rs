//! Account & usage — a port of the old app's `UsageSection` and
//! `UsageByModel`: lifetime totals, streaks, a year-long token heatmap with a
//! daily/weekly/cumulative view, and spend per model over a chosen range.
//! Every figure is derived from the daily series in `values.usage_days`;
//! nothing here is stubbed.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::IconName;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::setting::{SettingGroup, SettingItem, SettingPage};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::prelude::*;
use gpui_kit::{
    ClickEvent, FontWeight, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled, div, px, relative,
};

use crate::settings::values::{ModelUsage, Shared, Values};
use crate::settings::widgets;
use crate::theme::{self, color};
use gpui_kit::{App, Window};

const WEEKS: usize = 53;
const CELL: f32 = 11.0;
const CELL_GAP: f32 = 3.0;
const MONTH_NAMES: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
/// A cell with no activity — a touch lighter than the surface so the grid reads.
const EMPTY_CELL: u32 = 0x272b31;

// ------------------------------------------------------------------ dates ---

/// Today as a whole epoch day. The demo store keys days by offset from
/// today, so whole-day UTC arithmetic lines up with the local calendar the
/// old app used for its `YYYY-MM-DD` keys.
fn today() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| (d.as_secs() / 86_400) as i64)
}

/// Civil date from a whole epoch day — Howard Hinnant's `civil_from_days`,
/// in plain integer arithmetic so no calendar crate is needed.
fn civil(epoch_day: i64) -> (i64, u32, u32) {
    let z = epoch_day + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

/// `YYYY-MM-DD` for a whole epoch day.
fn date(epoch_day: i64) -> String {
    let (y, m, d) = civil(epoch_day);
    format!("{y:04}-{m:02}-{d:02}")
}

// ----------------------------------------------------------- formatters ---

/// 12.4M / 340.0K / 512 — magnitudes read at a glance; a raw nine-digit token
/// count does not. Same output as the old app's `compact`.
fn compact(n: u64) -> String {
    if n >= 1_000_000_000 {
        format!("{:.1}B", n as f64 / 1e9)
    } else if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

/// 1,234,567 — the insights rows show exact counts with separators.
fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let len = digits.len();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `$9.13` — three decimals under a dime, two above, as the old app did.
fn money(c: f64) -> String {
    if c < 0.1 {
        format!("${c:.3}")
    } else {
        format!("${c:.2}")
    }
}

/// The headline cost. A confirmed zero reads as "$0.00"; nothing priced yet
/// with only unpriced requests reads as "—", never as a fake zero.
fn cost_label(total: f64, unknown: bool, requests: u64) -> String {
    if requests == 0 {
        "$0.00".to_owned()
    } else if total == 0.0 && unknown {
        "—".to_owned()
    } else {
        format!("${total:.2}{}", if unknown { "+" } else { "" })
    }
}

/// Longest run of consecutive active days, and the run ending today (or
/// yesterday — a streak shouldn't read as broken before the day is out).
fn streaks(v: &Values) -> (u32, u32) {
    let active: BTreeSet<u32> = v
        .usage_days
        .iter()
        .filter(|d| d.requests > 0)
        .map(|d| d.offset)
        .collect();
    if active.is_empty() {
        return (0, 0);
    }
    let offsets: Vec<u32> = active.iter().copied().collect();
    let mut longest = 1;
    let mut run = 1;
    for w in offsets.windows(2) {
        if w[1] == w[0] + 1 {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 1;
        }
    }
    let has = |off: u32| active.contains(&off);
    if !has(0) && !has(1) {
        return (0, longest);
    }
    let mut cursor = if has(0) { 0 } else { 1 };
    let mut current = 0;
    while has(cursor) {
        current += 1;
        cursor += 1;
    }
    (current, longest)
}

// ----------------------------------------------------------------- model ---

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Daily = 0,
    Weekly = 1,
    Cumulative = 2,
}

fn mode_from(v: &Values) -> Mode {
    match v.usage_mode {
        1 => Mode::Weekly,
        2 => Mode::Cumulative,
        _ => Mode::Daily,
    }
}

/// One heatmap cell.
struct Cell {
    date: String,
    requests: u32,
    input: u64,
    output: u64,
    /// `None` when the day had only unpriced requests.
    cost: Option<f64>,
    /// Whatever the current view mode is colouring by.
    value: u64,
}

/// The heatmap computed for the current view mode, plus the window totals
/// behind the stat band and the insights rows.
struct Grid {
    cells: Vec<Cell>,
    max: u64,
    /// One label per month, at the column where that month first appears.
    months: Vec<(usize, &'static str)>,
    input: u64,
    output: u64,
    cost: f64,
    unknown: bool,
    requests: u64,
    active_days: u32,
    peak_tokens: u64,
    peak_date: String,
}

fn compute(v: &Values, mode: Mode) -> Grid {
    let today = today();
    let days = (WEEKS * 7) as i64;
    // Back up to Sunday so every column is a whole week (1970-01-01 was a
    // Thursday, hence the +4 in the weekday index).
    let start = today - (days - 1) - ((today - (days - 1) + 4).rem_euclid(7));
    let by_offset: BTreeMap<u32, &crate::settings::values::UsageDay> =
        v.usage_days.iter().map(|d| (d.offset, d)).collect();

    let mut cells = Vec::with_capacity(days as usize);
    for i in 0..days {
        let epoch = start + i;
        let offset = today - epoch;
        let day = if offset >= 0 {
            by_offset.get(&(offset as u32)).copied()
        } else {
            None
        };
        cells.push(Cell {
            date: date(epoch),
            requests: day.map_or(0, |d| d.requests),
            input: day.map_or(0, |d| d.input),
            output: day.map_or(0, |d| d.output),
            cost: day.and_then(|d| d.cost),
            value: day.map_or(0, |d| d.input + d.output),
        });
    }

    // Weekly gives every cell in a column its column total, so the heatmap
    // reads as week-over-week bands; cumulative is the running total, which
    // only ever climbs.
    match mode {
        Mode::Daily => {}
        Mode::Weekly => {
            for col in 0..WEEKS {
                let week = &mut cells[col * 7..col * 7 + 7];
                let total: u64 = week.iter().map(|c| c.input + c.output).sum();
                for c in week {
                    c.value = total;
                }
            }
        }
        Mode::Cumulative => {
            let mut running = 0_u64;
            for c in &mut cells {
                running += c.input + c.output;
                c.value = running;
            }
        }
    }

    let max = cells.iter().map(|c| c.value).max().unwrap_or(0);

    let mut months = Vec::new();
    let mut last_month: i64 = -1;
    for col in 0..WEEKS {
        let m = civil(start + (col * 7) as i64).1;
        if m as i64 != last_month {
            months.push((col, MONTH_NAMES[m as usize - 1]));
            last_month = m as i64;
        }
    }

    let mut input = 0;
    let mut output = 0;
    let mut cost = 0.0;
    let mut unknown = false;
    let mut requests = 0;
    let mut active_days = 0;
    let mut peak_tokens = 0;
    let mut peak_date = String::new();
    for c in &cells {
        input += c.input;
        output += c.output;
        match c.cost {
            Some(day_cost) => cost += day_cost,
            None => unknown = true,
        }
        requests += u64::from(c.requests);
        if c.requests > 0 {
            active_days += 1;
        }
        if c.input + c.output > peak_tokens {
            peak_tokens = c.input + c.output;
            peak_date = c.date.clone();
        }
    }

    Grid {
        cells,
        max,
        months,
        input,
        output,
        cost,
        unknown,
        requests,
        active_days,
        peak_tokens,
        peak_date,
    }
}

// ------------------------------------------------------------- elements ---

/// Blend `accent` over `base` by `t` (0..1), as a heatmap shade.
fn blend(accent: u32, base: u32, t: f32) -> u32 {
    let mix = |a: u32, b: u32| ((a as f32) * t + (b as f32) * (1.0 - t)).round() as u32;
    let (ar, ag, ab) = (accent >> 16 & 0xff, accent >> 8 & 0xff, accent & 0xff);
    let (br, bg, bb) = (base >> 16 & 0xff, base >> 8 & 0xff, base & 0xff);
    (mix(ar, br) << 16) | (mix(ag, bg) << 8) | mix(ab, bb)
}

/// One stat tile of the top band, with a divider to its left when not first.
fn tile(value: String, label: &'static str, divider: bool) -> impl IntoElement {
    v_flex()
        .flex_1()
        .items_center()
        .gap_1()
        .px_2()
        .py(px(14.0))
        .when(divider, |t| {
            t.border_l_1().border_color(color(theme::BORDER))
        })
        .child(
            div()
                .text_xl()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color(theme::TEXT))
                .child(value),
        )
        .child(div().text_xs().text_color(color(theme::MUTED)).child(label))
}

/// A small view-toggle chip: filled when selected, ghost when not.
fn chip(
    key: &'static str,
    label: &'static str,
    selected: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Button {
    let button = Button::new(key).label(label).small();
    let button = if selected {
        button.secondary()
    } else {
        button.ghost()
    };
    button.on_click(on_click)
}

/// The page: stat band, heatmap, spend per model, insights — the old
/// UsageSection + UsageByModel, in order.
pub fn page(values: &Shared) -> SettingPage {
    SettingPage::new("Account & usage")
        .icon(IconName::User)
        .description("Your account, and what Vivianne has spent across providers.")
        .group(SettingGroup::new().item(stat_band_item(values)))
        .group(activity_group(values))
        .group(by_model_group(values))
        .group(insights_group(values))
}

fn stat_band_item(values: &Shared) -> SettingItem {
    SettingItem::render({
        let values = values.clone();
        move |_options, _window, _cx| {
            let v = values.borrow();
            let grid = compute(&v, Mode::Daily);
            let (current, longest) = streaks(&v);
            let days = |n: u32| {
                if n == 1 {
                    "1 day".to_owned()
                } else {
                    format!("{n} days")
                }
            };
            let tiles = [
                (compact(grid.input + grid.output), "Lifetime tokens"),
                (compact(grid.peak_tokens), "Peak day"),
                (grid.active_days.to_string(), "Active days"),
                (days(current), "Current streak"),
                (days(longest), "Longest streak"),
            ];
            h_flex()
                .rounded(px(10.0))
                .border_1()
                .border_color(color(theme::BORDER))
                .overflow_hidden()
                .children(
                    tiles
                        .into_iter()
                        .enumerate()
                        .map(|(i, (value, label))| tile(value, label, i > 0)),
                )
                .into_any_element()
        }
    })
}

fn activity_group(values: &Shared) -> SettingGroup {
    let values_mode = values.clone();
    let toggle = SettingItem::render(move |_options, _window, _cx| {
        let mode = mode_from(&values_mode.borrow());
        let chip_for =
            |key: &'static str, label: &'static str, want: Mode, values: &Shared, mode: Mode| {
                let values = values.clone();
                chip(key, label, mode == want, move |_, _, cx| {
                    values.borrow_mut().usage_mode = want as u8;
                    cx.refresh_windows();
                })
            };
        h_flex()
            .justify_end()
            .gap_1()
            .child(chip_for(
                "usage-mode-daily",
                "Daily",
                Mode::Daily,
                &values_mode,
                mode,
            ))
            .child(chip_for(
                "usage-mode-weekly",
                "Weekly",
                Mode::Weekly,
                &values_mode,
                mode,
            ))
            .child(chip_for(
                "usage-mode-cumulative",
                "Cumulative",
                Mode::Cumulative,
                &values_mode,
                mode,
            ))
            .into_any_element()
    });
    SettingGroup::new()
        .title("Token activity")
        .item(toggle)
        .item(heatmap_item(values))
}

fn heatmap_item(values: &Shared) -> SettingItem {
    SettingItem::render({
        let values = values.clone();
        move |_options, _window, _cx| {
            let v = values.borrow();
            let grid = compute(&v, mode_from(&v));
            let max = grid.max;

            let mut grid_el = v_flex().gap_0p5();
            for r in 0..7 {
                let mut row = h_flex().gap(px(CELL_GAP));
                for col in 0..WEEKS {
                    let cell = &grid.cells[col * 7 + r];
                    let bg = if cell.value > 0 {
                        // Floor at 0.2 so a real-but-small day is still
                        // visibly distinct from an empty one.
                        let t = if max == 0 {
                            0.2
                        } else {
                            (cell.value as f32 / max as f32).max(0.2)
                        };
                        blend(theme::ACCENT, theme::BG, t)
                    } else {
                        EMPTY_CELL
                    };
                    let tip: SharedString = if cell.requests > 0 {
                        format!(
                            "{} — {} request{}, {} tokens",
                            cell.date,
                            cell.requests,
                            if cell.requests == 1 { "" } else { "s" },
                            thousands(cell.input + cell.output),
                        )
                    } else {
                        format!("{} — no activity", cell.date)
                    }
                    .into();
                    row = row.child(
                        div()
                            .id(SharedString::from(cell.date.clone()))
                            .size(px(CELL))
                            .rounded(px(2.0))
                            .bg(color(bg))
                            .hoverable_tooltip(move |_, cx| widgets::hover_tip(cx, tip.clone())),
                    );
                }
                grid_el = grid_el.child(row);
            }

            // Month axis — labels line up with the week each month starts in.
            let axis = div()
                .relative()
                .h(px(16.0))
                .mt(px(6.0))
                .children(grid.months.iter().map(|(col, name)| {
                    div()
                        .absolute()
                        .left(px(*col as f32 * (CELL + CELL_GAP)))
                        .text_xs()
                        .text_color(color(theme::MUTED))
                        .child(*name)
                }));

            let mut out = v_flex().child(grid_el).child(axis);
            if max == 0 {
                out = out.child(
                    div()
                        .text_xs()
                        .text_color(color(theme::MUTED))
                        .mt(px(6.0))
                        .child("No usage recorded yet — this fills in as you use real provider models."),
                );
            }
            out.into_any_element()
        }
    })
}

fn by_model_group(values: &Shared) -> SettingGroup {
    let values_range = values.clone();
    let range = SettingItem::render(move |_options, _window, _cx| {
        let current = values_range.borrow().usage_range.clone();
        let chip_for = |key: &'static str,
                        label: &'static str,
                        value: &'static str,
                        values: &Shared,
                        current: &str| {
            let values = values.clone();
            let selected = current == value;
            chip(key, label, selected, move |_, _, cx| {
                values.borrow_mut().usage_range = value.to_owned();
                cx.refresh_windows();
            })
        };
        h_flex()
            .justify_end()
            .gap_1()
            .child(chip_for(
                "usage-range-7",
                "7 days",
                "7",
                &values_range,
                &current,
            ))
            .child(chip_for(
                "usage-range-30",
                "30 days",
                "30",
                &values_range,
                &current,
            ))
            .child(chip_for(
                "usage-range-all",
                "All",
                "all",
                &values_range,
                &current,
            ))
            .into_any_element()
    });
    SettingGroup::new()
        .title("By model")
        .item(range)
        .item(model_rows_item(values))
}

/// Which model did the work, and what it cost. Models without a known price
/// are marked, not shown as free.
fn model_rows_item(values: &Shared) -> SettingItem {
    SettingItem::render({
        let values = values.clone();
        move |_options, _window, _cx| {
            let v = values.borrow();
            let mut rows: Vec<&ModelUsage> = v.model_usage.iter().collect();
            rows.sort_by(|a, b| (b.input + b.output).cmp(&(a.input + a.output)));
            if rows.is_empty() {
                return div()
                    .text_xs()
                    .text_color(color(theme::MUTED))
                    .child("Nothing in this range yet. Per-model usage starts counting from now.")
                    .into_any_element();
            }
            let max = rows
                .iter()
                .map(|r| r.input + r.output)
                .max()
                .unwrap_or(1)
                .max(1);
            let total_tokens: u64 = rows.iter().map(|r| r.input + r.output).sum();
            let total_cost: f64 = rows.iter().filter_map(|r| r.cost).sum();
            let unknown = rows.iter().any(|r| r.cost.is_none());

            let mut list = v_flex().gap_3();
            for r in rows {
                let tokens = r.input + r.output;
                let (cost_text, cost_tip) = match r.cost {
                    Some(c) => (
                        money(c),
                        "Cost at the model\u{2019}s listed price".to_owned(),
                    ),
                    None => (
                        "unpriced".to_owned(),
                        "This model has no known price, so its cost is partial or unknown"
                            .to_owned(),
                    ),
                };
                list = list.child(
                    v_flex()
                        .gap_1()
                        .child(
                            h_flex()
                                .items_baseline()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(color(theme::TEXT))
                                        .child(r.model.clone()),
                                )
                                .when(!r.provider.is_empty(), |name| {
                                    name.child(
                                        div()
                                            .text_xs()
                                            .text_color(color(theme::MUTED))
                                            .child(r.provider.clone()),
                                    )
                                })
                                .child(
                                    div()
                                        .font_family(theme::MONO)
                                        .text_xs()
                                        .text_color(color(theme::MUTED))
                                        .child(format!(
                                            "{} in · {} out",
                                            compact(r.input),
                                            compact(r.output)
                                        )),
                                )
                                .child(
                                    div()
                                        .id(SharedString::from(format!("usage-cost-{}", r.model)))
                                        .w(px(76.0))
                                        .text_right()
                                        .font_family(theme::MONO)
                                        .text_sm()
                                        .text_color(if r.cost.is_none() {
                                            color(theme::MUTED)
                                        } else {
                                            color(theme::TEXT)
                                        })
                                        .child(cost_text)
                                        .hoverable_tooltip(move |_, cx| {
                                            widgets::hover_tip(cx, cost_tip.clone())
                                        }),
                                ),
                        )
                        .child(
                            div()
                                .h(px(4.0))
                                .rounded_full()
                                .bg(color(EMPTY_CELL))
                                .overflow_hidden()
                                .child(
                                    div()
                                        .h_full()
                                        .w(relative(
                                            ((tokens as f32 / max as f32) * 100.0).max(2.0) / 100.0,
                                        ))
                                        .rounded_full()
                                        .bg(color(theme::ACCENT)),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(color(theme::MUTED))
                                .child(format!(
                                    "{} request{}",
                                    r.requests,
                                    if r.requests == 1 { "" } else { "s" }
                                )),
                        ),
                );
            }

            list = list.child(
                h_flex()
                    .justify_between()
                    .pt_2()
                    .border_t_1()
                    .border_color(color(theme::BORDER))
                    .child(
                        div()
                            .text_xs()
                            .text_color(color(theme::MUTED))
                            .child(format!(
                                "{} model{} · {} tokens",
                                v.model_usage.len(),
                                if v.model_usage.len() == 1 { "" } else { "s" },
                                compact(total_tokens)
                            )),
                    )
                    .child(
                        div()
                            .font_family(theme::MONO)
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(color(theme::TEXT))
                            .child(format!(
                                "{}{}",
                                money(total_cost),
                                if unknown { "+" } else { "" }
                            )),
                    ),
            );
            list.into_any_element()
        }
    })
}

fn insights_group(values: &Shared) -> SettingGroup {
    SettingGroup::new()
        .title("Activity insights")
        .item(insights_item(values))
}

fn insights_item(values: &Shared) -> SettingItem {
    SettingItem::render({
        let values = values.clone();
        move |_options, _window, _cx| {
            let v = values.borrow();
            let grid = compute(&v, Mode::Daily);
            let lifetime = grid.input + grid.output;
            let rows = [
                ("Total requests", thousands(grid.requests)),
                ("Input tokens", thousands(grid.input)),
                ("Output tokens", thousands(grid.output)),
                (
                    "Avg tokens / active day",
                    if grid.active_days > 0 {
                        compact(lifetime / u64::from(grid.active_days))
                    } else {
                        "—".to_owned()
                    },
                ),
                (
                    "Busiest day",
                    if grid.peak_date.is_empty() {
                        "—".to_owned()
                    } else {
                        grid.peak_date.clone()
                    },
                ),
                (
                    "Estimated cost",
                    cost_label(grid.cost, grid.unknown, grid.requests),
                ),
            ];

            let mut list = v_flex();
            for (i, (label, value)) in rows.into_iter().enumerate() {
                list = list.child(
                    h_flex()
                        .items_center()
                        .justify_between()
                        .gap_4()
                        .py_1()
                        .when(i > 0, |r| r.border_t_1().border_color(color(theme::BORDER)))
                        .child(div().text_sm().text_color(color(theme::MUTED)).child(label))
                        .child(
                            div()
                                .font_family(theme::MONO)
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(color(theme::TEXT))
                                .child(value),
                        ),
                );
            }
            if grid.unknown {
                list = list.child(
                    div()
                        .text_xs()
                        .text_color(color(theme::MUTED))
                        .mt(px(6.0))
                        .child(
                            "Cost is partial — some requests used a model without known pricing, so the real total is at least this much.",
                        ),
                );
            }
            list.into_any_element()
        }
    })
}
