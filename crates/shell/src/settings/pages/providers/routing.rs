//! Cost-aware routing (beta): work nobody reads (summaries, memory upkeep, read-only exploring) goes
//! to a cheap model; the chat stays on the one the user picked.

use gpui_kit::component::setting::{SettingField, SettingGroup, SettingItem};
use gpui_kit::prelude::*;
use gpui_kit::{App, SharedString, Window, div};
use vivianne_ai::host::ModelRef;
use vivianne_ai::inference::difficulty::Difficulty;
use vivianne_ai::inference::routing::{self, Config};

use crate::backend;
use crate::settings::provider_state::{PickableModel, Savings, token};
use crate::settings::values::Shared;
use crate::settings::widgets::{btn, keyed_input, muted, note, strong};

const PROBE: &str = "probe";

pub fn group(values: &Shared) -> SettingGroup {
    let enabled = values.borrow().providers.routing.as_ref().is_some_and(|c| c.enabled);
    let mut group = SettingGroup::new()
        .title("Cost-aware routing (beta)")
        .item(note("Small models are good enough for most background work. With this on, that work goes to the quick model; your replies, planning and anything that writes files or runs commands stay on the chat model."))
        .item(SettingItem::new("Send background work to the quick model", switch(values, Which::Enabled)))
        .item(SettingItem::new("Quick model", model_field(values, Slot::Quick)))
        .item(SettingItem::new("Match the model to the task", switch(values, Which::Auto)));
    if enabled {
        group = group
            .item(SettingItem::new("Easy: a lookup, a rename, a summary", model_field(values, Slot::Tier(0))))
            .item(SettingItem::new("Medium: a feature or fix in a few files", model_field(values, Slot::Tier(1))))
            .item(SettingItem::new("Hard: design, a subtle bug, many dependent steps", model_field(values, Slot::Tier(2))))
            .item(note("A level with no model uses the next one up, never a smaller one. Easy work with none set uses the quick model."))
            .item(try_it(values))
            .item(savings(values));
    } else {
        group = group.item(note("Turn routing on to choose models for each level."));
    }
    group
}

#[derive(Clone, Copy)]
enum Which {
    Enabled,
    Auto,
}

#[derive(Clone, Copy)]
enum Slot {
    Quick,
    Tier(usize),
}

fn switch(values: &Shared, which: Which) -> SettingField<bool> {
    let (read, write) = (values.clone(), values.clone());
    SettingField::switch(
        move |_| {
            let v = read.borrow();
            v.providers.routing.as_ref().is_some_and(|c| match which {
                Which::Enabled => c.enabled,
                Which::Auto => c.auto,
            })
        },
        move |on, cx| {
            change(&write, cx, |c| match which {
                Which::Enabled => c.enabled = on,
                Which::Auto => c.auto = on,
            });
        },
    )
}

/// A dropdown of the models the providers offer. The list is read the first time routing is shown.
fn model_field(values: &Shared, slot: Slot) -> SettingField<SharedString> {
    let (read, write, list) = (values.clone(), values.clone(), values.clone());
    let models = list.borrow().providers.models.clone();
    let mut options = vec![(SharedString::default(), SharedString::from("Not set"))];
    options.extend(models.iter().map(|m| (SharedString::from(m.token()), SharedString::from(format!("{} ({})", m.name, m.provider)))));
    SettingField::dropdown(
        options,
        move |_| {
            let v = read.borrow();
            let current = v.providers.routing.as_ref().and_then(|c| slot_model(c, slot));
            SharedString::from(current.map(|m| token(&m.provider, &m.id)).unwrap_or_default())
        },
        move |picked, cx| {
            let model = write.borrow().providers.models.iter().find(|m| m.token() == picked.as_ref()).map(as_model_ref);
            change(&write, cx, |c| match slot {
                Slot::Quick => c.quick = model,
                Slot::Tier(i) => c.tiers[i] = model,
            });
        },
    )
}

fn slot_model(config: &Config, slot: Slot) -> Option<&ModelRef> {
    match slot {
        Slot::Quick => config.quick.as_ref(),
        Slot::Tier(i) => config.tiers[i].as_ref(),
    }
}

fn as_model_ref(m: &PickableModel) -> ModelRef {
    ModelRef { id: m.id.clone(), name: m.name.clone(), api: m.api.clone(), provider: m.provider.clone(), ..ModelRef::default() }
}

/// Edits the routing setting in memory, saves it, and refreshes the savings meter.
fn change(values: &Shared, cx: &mut App, edit: impl FnOnce(&mut Config)) {
    let config = {
        let mut v = values.borrow_mut();
        let Some(config) = v.providers.routing.as_mut() else { return };
        edit(config);
        config.clone()
    };
    cx.refresh_windows();
    let values = values.clone();
    backend::run(cx, async move { routing::save_config(&config).await }, move |result, cx| {
        if let Err(why) = result {
            values.borrow_mut().providers.error = Some(why);
        }
        load_savings(&values, cx);
        cx.refresh_windows();
    });
}

/// Reads the models once, when routing is first shown.
fn ensure_models(values: &Shared, cx: &mut App) {
    {
        let mut v = values.borrow_mut();
        if !v.providers.models.is_empty() || v.providers.is_busy("models") {
            return;
        }
        v.providers.start("models");
    }
    let values = values.clone();
    backend::run(cx, async { vivianne_ai::host::discover_models(false).await }, move |found, cx| {
        {
            let mut v = values.borrow_mut();
            v.providers.models = found
                .into_iter()
                .map(|m| PickableModel { provider: m.provider, id: m.id, name: m.name, api: m.api })
                .collect();
            v.providers.finish("models");
        }
        load_savings(&values, cx);
        cx.refresh_windows();
    });
}

fn load_savings(values: &Shared, cx: &mut App) {
    let smart = vivianne_providers::env::selection().ok().map(|s| ModelRef { id: s.model.clone(), name: s.model, provider: s.provider, ..ModelRef::default() });
    let Some(smart) = smart else { return };
    let values = values.clone();
    backend::run(cx, async move { routing::cmd_routing_savings(&smart, Some(30)).await }, move |answer, cx| {
        if let Ok(json) = answer {
            values.borrow_mut().providers.savings = parse_savings(&json);
        }
        cx.refresh_windows();
    });
}

fn parse_savings(json: &serde_json::Value) -> Option<Savings> {
    if !json.get("configured")?.as_bool()? {
        return None;
    }
    let s = json.get("savings")?;
    let name = |who: &str| json.pointer(&format!("/{who}/name")).and_then(|n| n.as_str()).unwrap_or_default().to_owned();
    Some(Savings {
        requests: s.get("requests")?.as_u64().and_then(|n| u32::try_from(n).ok()).unwrap_or_default(),
        input: s.get("input")?.as_u64().unwrap_or_default(),
        output: s.get("output")?.as_u64().unwrap_or_default(),
        actual_usd: s.get("actualUsd").and_then(serde_json::Value::as_f64),
        would_usd: s.get("wouldUsd").and_then(serde_json::Value::as_f64),
        saved_usd: s.get("savedUsd").and_then(serde_json::Value::as_f64),
        quick_name: name("quick"),
        smart_name: name("smart"),
    })
}

fn try_it(values: &Shared) -> SettingItem {
    let values = values.clone();
    SettingItem::render(move |_options, window: &mut Window, cx| {
        ensure_models(&values, cx);
        let input = keyed_input(
            PROBE,
            &values,
            |v| v.providers.draft(PROBE),
            |v, x| v.providers.set_draft(PROBE, x),
            "Type a task to see how it would be read",
            window,
            cx,
        );
        let verdict = values.borrow().providers.verdict;
        let reading = verdict.map(|v| format!("Reads as {}, decided by {}.", level_name(v.level), v.by));
        let ask = values.clone();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(strong("Try it"))
            .child(input)
            .child(btn("probe-read", "Read it", move |_, _, cx| {
                let text = ask.borrow().providers.draft(PROBE);
                if text.trim().is_empty() {
                    return;
                }
                let values = ask.clone();
                backend::run(cx, async move { vivianne_ai::inference::difficulty::classify(&text).await }, move |verdict, cx| {
                    values.borrow_mut().providers.verdict = Some(verdict);
                    cx.refresh_windows();
                });
            }))
            .children(reading.map(muted))
            .into_any_element()
    })
}

fn level_name(level: Difficulty) -> &'static str {
    match level {
        Difficulty::Easy => "easy",
        Difficulty::Medium => "medium",
        Difficulty::Hard => "hard",
    }
}

fn savings(values: &Shared) -> SettingItem {
    let values = values.clone();
    SettingItem::render(move |_options, _window, _cx| {
        let v = values.borrow();
        let text = match (&v.providers.savings, v.providers.routing.as_ref().and_then(|c| c.quick.as_ref())) {
            (_, None) => "Choose a quick model to start counting.".to_owned(),
            (None, Some(quick)) => format!("Nothing has been routed to {} in the last 30 days.", quick.name),
            (Some(s), _) if s.requests == 0 => format!("Nothing has been routed to {} in the last 30 days.", s.quick_name),
            (Some(s), _) => savings_text(s),
        };
        div().flex().flex_col().gap_1().child(strong("Last 30 days")).child(muted(text)).into_any_element()
    })
}

fn savings_text(s: &Savings) -> String {
    let calls = if s.requests == 1 { "call" } else { "calls" };
    let head = format!("Routed {} {calls} ({} in, {} out) to {}.", s.requests, compact(s.input), compact(s.output), s.quick_name);
    match (s.actual_usd, s.would_usd, s.saved_usd) {
        (Some(actual), Some(would), Some(saved)) => format!(
            "{head} That cost {}; on {} it would have been about {}, so roughly {} saved.",
            money(actual),
            s.smart_name,
            money(would),
            money(saved)
        ),
        _ => format!("{head} One of the two models has no known price, so there is no dollar figure to show."),
    }
}

fn compact(n: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let f = n as f64;
    if n >= 1_000_000 {
        format!("{:.1}M", f / 1e6)
    } else if n >= 1_000 {
        format!("{:.1}K", f / 1e3)
    } else {
        n.to_string()
    }
}

fn money(usd: f64) -> String {
    if usd < 0.1 { format!("${usd:.3}") } else { format!("${usd:.2}") }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn counts_are_shortened_and_small_costs_keep_their_digits() {
        assert_eq!(compact(950), "950");
        assert_eq!(compact(12_500), "12.5K");
        assert_eq!(compact(3_400_000), "3.4M");
        assert_eq!(money(0.034), "$0.034");
        assert_eq!(money(2.5), "$2.50");
    }

    #[test]
    fn the_savings_answer_is_read_and_an_unconfigured_one_is_nothing() {
        let answer = json!({
            "configured": true, "quick": {"name": "Nano"}, "smart": {"name": "Big"},
            "savings": {"requests": 40, "input": 1000, "output": 200, "actualUsd": 0.3, "wouldUsd": 2.5, "savedUsd": 2.2}
        });
        let s = parse_savings(&answer).unwrap_or_default();
        assert_eq!((s.requests, s.quick_name.as_str()), (40, "Nano"));
        assert!(savings_text(&s).contains("$2.20 saved"));
        assert!(parse_savings(&json!({"configured": false})).is_none());
    }
}
