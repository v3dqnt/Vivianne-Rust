//! Cost-aware routing (beta): work nobody reads (summaries, memory upkeep, read-only exploring) goes
//! to a cheap model; the chat stays on the one the user picked.

use gpui_kit::component::setting::{SettingField, SettingGroup, SettingItem};
use gpui_kit::{App, SharedString};
use vivianne_ai::host::ModelRef;
use vivianne_ai::inference::routing::{self, Config};

use crate::backend;
use crate::settings::provider_state::{PickableModel, token};
use crate::settings::values::Shared;
use crate::settings::widgets::note;

const PROBE: &str = "probe";

pub fn group(values: &Shared) -> SettingGroup {
    let enabled = values
        .borrow()
        .providers
        .routing
        .as_ref()
        .is_some_and(|c| c.enabled);
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
            ;
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
    options.extend(models.iter().map(|m| {
        (
            SharedString::from(m.token()),
            SharedString::from(format!("{} ({})", m.name, m.provider)),
        )
    }));
    SettingField::dropdown(
        options,
        move |_| {
            let v = read.borrow();
            let current = v
                .providers
                .routing
                .as_ref()
                .and_then(|c| slot_model(c, slot));
            SharedString::from(
                current
                    .map(|m| token(&m.provider, &m.id))
                    .unwrap_or_default(),
            )
        },
        move |picked, cx| {
            let model = write
                .borrow()
                .providers
                .models
                .iter()
                .find(|m| m.token() == picked.as_ref())
                .map(as_model_ref);
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
    ModelRef {
        id: m.id.clone(),
        name: m.name.clone(),
        api: m.api.clone(),
        provider: m.provider.clone(),
        ..ModelRef::default()
    }
}

/// Edits the routing setting in memory and saves it.
fn change(values: &Shared, cx: &mut App, edit: impl FnOnce(&mut Config)) {
    let config = {
        let mut v = values.borrow_mut();
        let Some(config) = v.providers.routing.as_mut() else {
            return;
        };
        edit(config);
        config.clone()
    };
    cx.refresh_windows();
    let values = values.clone();
    backend::run(
        cx,
        async move { routing::save_config(&config).await },
        move |result, cx| {
            if let Err(why) = result {
                values.borrow_mut().providers.error = Some(why);
            }
            cx.refresh_windows();
        },
    );
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
    backend::run(
        cx,
        async { vivianne_ai::host::discover_models(false).await },
        move |found, cx| {
            {
                let mut v = values.borrow_mut();
                v.providers.models = found
                    .into_iter()
                    .map(|m| PickableModel {
                        provider: m.provider,
                        id: m.id,
                        name: m.name,
                        api: m.api,
                    })
                    .collect();
                v.providers.finish("models");
            }
            cx.refresh_windows();
        },
    );
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
    if usd < 0.1 {
        format!("${usd:.3}")
    } else {
        format!("${usd:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_are_shortened_and_small_costs_keep_their_digits() {
        assert_eq!(compact(950), "950");
        assert_eq!(compact(12_500), "12.5K");
        assert_eq!(compact(3_400_000), "3.4M");
        assert_eq!(money(0.034), "$0.034");
        assert_eq!(money(2.5), "$2.50");
    }
}
