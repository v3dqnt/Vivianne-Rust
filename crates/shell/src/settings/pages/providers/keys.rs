//! API keys: one per provider, plus extra keys that take over when one is rate limited or out of credit.

use gpui_kit::base::h_flex;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::setting::{SettingGroup, SettingItem};
use gpui_kit::prelude::*;
use gpui_kit::{App, SharedString, Styled, Window, div};
use vivianne_ai::ai::auth::{CredentialStatus, PoolEntryInfo, RotationStrategy};
use vivianne_ai::providers::KeySource;

use super::ChatGptCall;
use crate::backend;
use crate::settings::provider_state::{CLOUD_PROVIDERS, Pane, provider_name};
use crate::settings::values::Shared;
use crate::settings::widgets::{btn, dot, keyed_input, keyed_secret, muted, pill, strong};
use crate::theme;

const KEY_DRAFT: &str = "key";
const POOL_LABEL: &str = "pool-label";
const POOL_KEY: &str = "pool-key";

pub fn group(values: &Shared) -> SettingGroup {
    let values = values.clone();
    SettingGroup::new()
        .title("API keys")
        .item(SettingItem::render(move |_options, window, cx| view(&values, window, cx).into_any_element()))
}

fn view(values: &Shared, window: &mut Window, cx: &mut App) -> impl IntoElement + use<> {
    let open = match &values.borrow().providers.pane {
        Pane::Cloud(id) => Some(id.clone()),
        _ => None,
    };
    let rows: Vec<_> = CLOUD_PROVIDERS.iter().map(|(id, name)| row(values, id, name, open.as_deref() == Some(*id))).collect();
    let editor = open.map(|id| editor(values, &id, window, cx));
    div().flex().flex_col().gap_2().children(rows).children(editor)
}

fn source_label(source: Option<KeySource>) -> &'static str {
    match source {
        Some(KeySource::Saved) => "Key saved",
        Some(KeySource::Environment) => "From an environment variable",
        _ => "No key",
    }
}

fn row(values: &Shared, id: &'static str, name: &'static str, open: bool) -> impl IntoElement + use<> {
    let (source, connected) = {
        let v = values.borrow();
        (v.providers.sources.get(id).copied().flatten(), v.providers.connected(id))
    };
    let toggle = values.clone();
    h_flex()
        .justify_between()
        .items_center()
        .gap_4()
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(dot(if connected { theme::OK } else { theme::MUTED }))
                .child(strong(name))
                .child(muted(source_label(source))),
        )
        .child(Button::new(SharedString::from(format!("provider-{id}"))).label(if open { "Close" } else { "Edit" }).small().ghost().on_click(
            move |_, _, cx| {
                if open {
                    toggle.borrow_mut().providers.pane = Pane::Nothing;
                } else {
                    open_provider(&toggle, id, cx);
                }
                cx.refresh_windows();
            },
        ))
}

/// Opens a provider's editor and reads its extra keys.
fn open_provider(values: &Shared, id: &str, cx: &mut App) {
    {
        let mut v = values.borrow_mut();
        v.providers.pane = Pane::Cloud(id.to_owned());
        v.providers.clear_draft(KEY_DRAFT);
        v.providers.clear_draft(POOL_LABEL);
        v.providers.clear_draft(POOL_KEY);
        v.providers.pool.clear();
        v.providers.error = None;
    }
    reload_pool(values, id, cx);
}

fn editor(values: &Shared, id: &str, window: &mut Window, cx: &mut App) -> impl IntoElement + use<> {
    let connected = values.borrow().providers.connected(id);
    let saved = values.borrow().providers.sources.get(id).copied().flatten() == Some(KeySource::Saved);
    let key = keyed_secret(
        KEY_DRAFT,
        values,
        |v| v.providers.draft(KEY_DRAFT),
        |v, x| v.providers.set_draft(KEY_DRAFT, x),
        if connected { "Paste a new key to replace it" } else { "Paste the API key" },
        window,
        cx,
    );
    let (save, remove) = (values.clone(), values.clone());
    let (save_id, remove_id) = (id.to_owned(), id.to_owned());
    let mut buttons = h_flex().gap_2().child(btn("key-save", "Save", move |_, _, cx| save_key(&save, &save_id, cx)));
    if saved {
        buttons = buttons.child(Button::new("key-remove").label("Remove").ghost().on_click(move |_, _, cx| delete_key(&remove, &remove_id, cx)));
    }
    let pool = connected.then(|| pool(values, id, window, cx));
    div()
        .flex()
        .flex_col()
        .gap_2()
        .p_3()
        .rounded_md()
        .border_1()
        .border_color(theme::color(theme::BORDER))
        .child(strong(format!("{} key", provider_name(id))))
        .child(key)
        .child(buttons)
        .children(pool)
}

fn pool(values: &Shared, id: &str, window: &mut Window, cx: &mut App) -> impl IntoElement + use<> {
    let (entries, strategy) = {
        let v = values.borrow();
        (v.providers.pool.clone(), v.providers.strategy)
    };
    let any_cooling = entries.iter().any(|e| e.in_cooldown);
    let rows: Vec<_> = entries.iter().map(|e| pool_row(values, id, e)).collect();
    let label = keyed_input(
        POOL_LABEL,
        values,
        |v| v.providers.draft(POOL_LABEL),
        |v, x| v.providers.set_draft(POOL_LABEL, x),
        "Label, for example backup key",
        window,
        cx,
    );
    let key = keyed_secret(POOL_KEY, values, |v| v.providers.draft(POOL_KEY), |v, x| v.providers.set_draft(POOL_KEY, x), "Paste another API key", window, cx);
    let (add, clear) = (values.clone(), values.clone());
    let (add_id, clear_id) = (id.to_owned(), id.to_owned());
    let mut footer = h_flex().gap_2().items_center().child(btn("pool-add", "Add key", move |_, _, cx| add_pool_key(&add, &add_id, cx)));
    if any_cooling {
        footer = footer.child(Button::new("pool-clear").label("Clear cooldowns").ghost().on_click(move |_, _, cx| {
            let hub = hub();
            let (values, id) = (clear.clone(), clear_id.clone());
            if let Some(hub) = hub {
                let again = id.clone();
                backend::run(cx, async move { hub.reset_cooldowns(&again).await }, move |(), cx| reload_pool(&values, &id, cx));
            }
        }));
    }
    div()
        .flex()
        .flex_col()
        .gap_2()
        .pt_2()
        .child(strong("Extra keys"))
        .child(muted("Add more than one key and Vivianne takes the next when one is rate limited or out of credit, before switching to another provider."))
        .children(rows)
        .child(strategy_row(values, id, strategy, entries.len() > 1))
        .child(label)
        .child(key)
        .child(footer)
}

fn pool_row(values: &Shared, provider: &str, entry: &PoolEntryInfo) -> impl IntoElement + use<> {
    let removable = entry.source == "pool";
    let (remove, provider_id, entry_id) = (values.clone(), provider.to_owned(), entry.id.clone());
    let mut row = h_flex().justify_between().items_center().gap_3().child(
        h_flex().gap_2().items_center().child(strong(entry.label.clone())).child(pill(SharedString::from(status_text(entry)))),
    );
    if removable {
        row = row.child(Button::new(SharedString::from(format!("pool-remove-{}", entry.id))).label("Remove").small().ghost().on_click(move |_, _, cx| {
            if let Some(hub) = hub() {
                let (values, id, key_id) = (remove.clone(), provider_id.clone(), entry_id.clone());
                let (ask_id, ask_key) = (id.clone(), key_id);
                backend::run(
                    cx,
                    async move { hub.remove_pool_key(&ask_id, &ask_key).await },
                    move |result, cx| {
                        finish(&values, result, cx);
                        reload_pool(&values, &id, cx);
                    },
                );
            }
        }));
    }
    row
}

fn status_text(entry: &PoolEntryInfo) -> String {
    if !entry.in_cooldown {
        return if entry.request_count > 0 { format!("{} requests", entry.request_count) } else { "Ready".to_owned() };
    }
    match entry.status {
        CredentialStatus::QuotaOrBilling => "Out of credit, cooling down".to_owned(),
        CredentialStatus::Retryable => "Rate limited, cooling down".to_owned(),
        _ => "Failed, cooling down".to_owned(),
    }
}

/// Which extra key is used first. Only matters with more than one key.
fn strategy_row(values: &Shared, provider: &str, current: RotationStrategy, enabled: bool) -> impl IntoElement + use<> {
    const ORDER: [(RotationStrategy, &str); 4] = [
        (RotationStrategy::FillFirst, "Use one until it fails"),
        (RotationStrategy::RoundRobin, "Take turns"),
        (RotationStrategy::LeastUsed, "Prefer the idlest"),
        (RotationStrategy::Random, "Random"),
    ];
    let buttons = ORDER.iter().map(|(strategy, label)| {
        let (values, provider, strategy) = (values.clone(), provider.to_owned(), *strategy);
        let selected = strategy == current;
        let mut button = Button::new(SharedString::from(format!("strategy-{label}"))).label(*label).small();
        if !selected {
            button = button.ghost();
        }
        button.on_click(move |_, _, cx| {
            if !enabled || selected {
                return;
            }
            if let Some(hub) = hub() {
                values.borrow_mut().providers.strategy = strategy;
                let ask = provider.clone();
                backend::run(cx, async move { hub.set_strategy(&ask, strategy).await }, |(), cx| cx.refresh_windows());
            }
        })
    });
    h_flex().gap_1().items_center().child(muted("Order")).children(buttons)
}

// ── Actions ─────────────────────────────────────────────────────────────────

fn hub() -> Option<std::sync::Arc<vivianne_ai::providers::Hub>> {
    backend::get().map(|b| b.hub.clone())
}

/// Shows a failure, or just redraws.
fn finish(values: &Shared, result: Result<(), String>, cx: &mut App) {
    if let Err(why) = result {
        values.borrow_mut().providers.error = Some(why);
    }
    cx.refresh_windows();
}

fn save_key(values: &Shared, id: &str, cx: &mut App) {
    let key = values.borrow().providers.draft(KEY_DRAFT);
    let Some(hub) = hub() else { return };
    let (values, id) = (values.clone(), id.to_owned());
    let ask = id.clone();
    backend::run(cx, async move { hub.save_key(&ask, &key).await }, move |result, cx| {
        if result.is_ok() {
            values.borrow_mut().providers.clear_draft(KEY_DRAFT);
        }
        finish(&values, result, cx);
        reload_sources(&values, cx);
        reload_pool(&values, &id, cx);
    });
}

fn delete_key(values: &Shared, id: &str, cx: &mut App) {
    let Some(hub) = hub() else { return };
    let (values, id) = (values.clone(), id.to_owned());
    let ask = id.clone();
    backend::run(cx, async move { hub.delete_key(&ask).await }, move |result, cx| {
        finish(&values, result, cx);
        reload_sources(&values, cx);
        reload_pool(&values, &id, cx);
    });
}

fn add_pool_key(values: &Shared, id: &str, cx: &mut App) {
    let (label, key) = {
        let v = values.borrow();
        (v.providers.draft(POOL_LABEL), v.providers.draft(POOL_KEY))
    };
    let Some(hub) = hub() else { return };
    let (values, id) = (values.clone(), id.to_owned());
    let ask = id.clone();
    backend::run(cx, async move { hub.add_pool_key(&ask, &label, &key).await }, move |result, cx| {
        if result.is_ok() {
            let mut v = values.borrow_mut();
            v.providers.clear_draft(POOL_LABEL);
            v.providers.clear_draft(POOL_KEY);
        }
        finish(&values, result, cx);
        reload_pool(&values, &id, cx);
    });
}

/// Reads one provider's keys and how they are taken in turn.
fn reload_pool(values: &Shared, id: &str, cx: &mut App) {
    let Some(hub) = hub() else { return };
    let (values, id) = (values.clone(), id.to_owned());
    let ask = id.clone();
    backend::run(cx, async move { hub.pool(&ask).await }, move |(entries, strategy), cx| {
        let mut v = values.borrow_mut();
        if v.providers.pane == Pane::Cloud(id) {
            v.providers.pool = entries;
            v.providers.strategy = strategy;
        }
        drop(v);
        cx.refresh_windows();
    });
}

/// Asks again which providers have a key.
pub(super) fn reload_sources(values: &Shared, cx: &mut App) {
    let Some(hub) = hub() else { return };
    let mut ids: Vec<String> = CLOUD_PROVIDERS.iter().map(|(id, _)| (*id).to_owned()).collect();
    ids.extend(values.borrow().providers.custom.iter().map(|c| c.id.clone()));
    let values = values.clone();
    backend::run(cx, async move { hub.sources(&ids).await }, move |sources, cx| {
        {
            let mut v = values.borrow_mut();
            v.providers.sources = sources.into_iter().map(|(id, source)| (id, Some(source))).collect();
            let keyed: Vec<String> = v.providers.custom.iter().filter(|c| v.providers.connected(&c.id)).map(|c| c.id.clone()).collect();
            v.providers.custom_keyed = keyed;
        }
        cx.refresh_windows();
    });
}

pub(super) fn run_chatgpt(values: &Shared, cx: &mut App, call: ChatGptCall) {
    let Some(hub) = hub() else { return };
    values.borrow_mut().providers.start("chatgpt");
    cx.refresh_windows();
    let values = values.clone();
    backend::run(
        cx,
        async move {
            match call {
                ChatGptCall::SignIn => hub.chatgpt_sign_in().await,
                ChatGptCall::SignOut => hub.chatgpt_sign_out().await,
            }
        },
        move |result, cx| {
            {
                let mut v = values.borrow_mut();
                v.providers.finish("chatgpt");
                match result {
                    Ok(status) => v.providers.chatgpt = Some(status),
                    Err(why) => v.providers.error = Some(why),
                }
            }
            cx.refresh_windows();
        },
    );
}
