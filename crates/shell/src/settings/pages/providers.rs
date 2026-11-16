//! Providers: API keys, key pools, the ChatGPT sign-in, model routing, coding
//! agents and user-defined providers.

use gpui_kit::base::h_flex;
use gpui_kit::component::IconName;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::setting::{SettingGroup, SettingItem, SettingPage};
use gpui_kit::prelude::*;
use gpui_kit::{App, SharedString, Styled, Window, div};

use crate::settings::values::{
    AGENT_MISSING, AGENT_READY, AGENT_SETUP, CodingAgent, CustomProvider, KEY_FAIL, KEY_QUOTA, KEY_RATE, PoolKey, Shared,
};
use crate::settings::widgets::{btn, field_dropdown, field_switch, keyed_input, muted, mono, note, pill, strong};

/// The providers that take an API key, as (id, name).
const PROVIDERS: [(&str, &str); 8] = [
    ("anthropic", "Anthropic"),
    ("openai", "OpenAI"),
    ("google", "Google"),
    ("openrouter", "OpenRouter"),
    ("mistral", "Mistral"),
    ("groq", "Groq"),
    ("xai", "xAI"),
    ("deepseek", "DeepSeek"),
];

const ROTATIONS: [(&str, &str); 2] = [("fill_first", "Use the first until it runs out"), ("round_robin", "Take turns")];
const KEY_DRAFT: &str = "provider-key";

pub fn page(values: &Shared) -> SettingPage {
    SettingPage::new("Providers")
        .icon(IconName::Globe)
        .description("Where the models come from, and the keys that unlock them.")
        .group(keys_group(values))
        .group(pool_group(values))
        .group(chatgpt_group(values))
        .group(routing_group(values))
        .group(agents_group(values))
        .group(custom_group(values))
}

/// A key shown without its middle: enough to tell two apart.
fn masked(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= 8 {
        return "••••".to_owned();
    }
    let head: String = chars[..4].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}••••{tail}")
}

fn provider_name(id: &str) -> &str {
    PROVIDERS.iter().find(|(p, _)| *p == id).map_or(id, |(_, name)| name)
}

fn keys_group(values: &Shared) -> SettingGroup {
    let values = values.clone();
    SettingGroup::new()
        .title("API keys")
        .description("Pick a provider, paste its key, save.")
        .item(SettingItem::render(move |_options, window, cx| {
            let (open, saved) = {
                let v = values.borrow();
                (v.open_provider.clone(), v.api_keys.clone())
            };
            let rows = PROVIDERS.iter().enumerate().map(|(index, (id, name))| {
                let select = values.clone();
                let selected = open == *id;
                let status = saved.get(*id).map_or_else(|| "No key".to_owned(), |k| masked(k));
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(h_flex().gap_2().items_center().child(strong(*name)).child(muted(status)))
                    .child(Button::new(("provider", index)).label(if selected { "Editing" } else { "Edit" }).small().ghost().on_click(
                        move |_, _, cx| {
                            let mut v = select.borrow_mut();
                            v.open_provider = (*id).to_owned();
                            v.key_drafts.remove(KEY_DRAFT);
                            cx.refresh_windows();
                        },
                    ))
            });
            let editor = (!open.is_empty()).then(|| key_editor(&values, &open, window, cx));
            div().flex().flex_col().gap_3().children(rows).children(editor).into_any_element()
        }))
}

fn key_editor(values: &Shared, id: &str, window: &mut Window, cx: &mut App) -> impl IntoElement + use<> {
    let input = keyed_input(
        KEY_DRAFT,
        values,
        |v| v.key_drafts.get(KEY_DRAFT).cloned().unwrap_or_default(),
        |v, x| {
            v.key_drafts.insert(KEY_DRAFT.to_owned(), x);
        },
        "Paste the key",
        window,
        cx,
    );
    let (save, remove) = (values.clone(), values.clone());
    let (save_id, remove_id) = (id.to_owned(), id.to_owned());
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(strong(format!("{} key", provider_name(id))))
        .child(input)
        .child(
            h_flex()
                .gap_2()
                .child(btn("key-save", "Save", move |_, _, cx| {
                    let mut v = save.borrow_mut();
                    let draft = v.key_drafts.remove(KEY_DRAFT).unwrap_or_default();
                    if !draft.trim().is_empty() {
                        v.api_keys.insert(save_id.clone(), draft.trim().to_owned());
                    }
                    cx.refresh_windows();
                }))
                .child(btn("key-remove", "Remove", move |_, _, cx| {
                    remove.borrow_mut().api_keys.remove(&remove_id);
                    cx.refresh_windows();
                })),
        )
}

fn state_label(state: u8) -> &'static str {
    match state {
        KEY_QUOTA => "Out of credit",
        KEY_RATE => "Rate limited",
        KEY_FAIL => "Failed",
        _ => "Ready",
    }
}

fn pool_group(values: &Shared) -> SettingGroup {
    let values = values.clone();
    SettingGroup::new()
        .title("Key pool")
        .description("Several keys for one provider; when one runs out the next takes over.")
        .item(SettingItem::new(
            "Order",
            field_dropdown(
                &values,
                &ROTATIONS,
                |v| v.rotation.get("openai").cloned().unwrap_or_else(|| "fill_first".into()),
                |v, x| {
                    v.rotation.insert("openai".into(), x);
                },
            ),
        ))
        .item(SettingItem::render(move |_options, _window, _cx| {
            let keys: Vec<PoolKey> = values.borrow().pool.get("openai").cloned().unwrap_or_default();
            if keys.is_empty() {
                return muted("No keys in the pool.").into_any_element();
            }
            let rows = keys.into_iter().enumerate().map(|(index, k)| {
                let remove = values.clone();
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(h_flex().gap_2().items_center().child(strong(k.label.clone())).child(pill(state_label(k.state))))
                            .child(mono(format!("{} · {} requests", masked(&k.key), k.requests))),
                    )
                    .child(Button::new(("pool-remove", index)).label("Remove").small().ghost().on_click(move |_, _, cx| {
                        if let Some(list) = remove.borrow_mut().pool.get_mut("openai") {
                            if index < list.len() {
                                list.remove(index);
                            }
                        }
                        cx.refresh_windows();
                    }))
            });
            div().flex().flex_col().gap_3().children(rows).into_any_element()
        }))
}

fn chatgpt_group(values: &Shared) -> SettingGroup {
    let values = values.clone();
    SettingGroup::new()
        .title("ChatGPT")
        .description("Use a ChatGPT plan instead of an API key.")
        .item(SettingItem::render(move |_options, _window, _cx| {
            let (connected, email) = {
                let v = values.borrow();
                (v.chatgpt_connected, v.chatgpt_email.clone())
            };
            let toggle = values.clone();
            let label = if connected { "Sign out" } else { "Sign in" };
            h_flex()
                .justify_between()
                .items_center()
                .child(muted(if connected { format!("Signed in as {email}") } else { "Not signed in".to_owned() }))
                .child(Button::new("chatgpt").label(label).on_click(move |_, _, cx| {
                    let mut v = toggle.borrow_mut();
                    v.chatgpt_connected = !v.chatgpt_connected;
                    cx.refresh_windows();
                }))
                .into_any_element()
        }))
}

fn routing_group(values: &Shared) -> SettingGroup {
    SettingGroup::new()
        .title("Routing")
        .description("Send easy questions to a cheaper model and hard ones to a stronger one.")
        .item(SettingItem::new("Match the model to the task", field_switch(values, |v| v.routing_enabled, |v, on| v.routing_enabled = on)))
        .item(SettingItem::new("Pick automatically", field_switch(values, |v| v.routing_auto, |v, on| v.routing_auto = on)))
        .item(note("Without automatic picking, choose a model for each difficulty from the model picker."))
}

fn agent_state(state: u8) -> &'static str {
    match state {
        AGENT_READY => "Ready",
        AGENT_SETUP => "Needs setup",
        AGENT_MISSING => "Not installed",
        _ => "",
    }
}

fn agents_group(values: &Shared) -> SettingGroup {
    let values = values.clone();
    SettingGroup::new()
        .title("Coding agents")
        .description("Other coding agents Vivianne can hand work to.")
        .item(SettingItem::render(move |_options, _window, _cx| {
            let (agents, all) = {
                let v = values.borrow();
                (v.agents.clone(), v.agents_show_all)
            };
            let shown: Vec<(usize, CodingAgent)> =
                agents.into_iter().enumerate().filter(|(_, a)| all || a.state != AGENT_MISSING).collect();
            let rows = shown.into_iter().map(|(index, a)| agent_row(&values, index, &a));
            let more = values.clone();
            div()
                .flex()
                .flex_col()
                .gap_3()
                .children(rows)
                .child(Button::new("agents-all").label(if all { "Show fewer" } else { "Show all" }).ghost().small().on_click(move |_, _, cx| {
                    let mut v = more.borrow_mut();
                    v.agents_show_all = !v.agents_show_all;
                    cx.refresh_windows();
                }))
                .into_any_element()
        }))
}

fn agent_row(values: &Shared, index: usize, agent: &CodingAgent) -> impl IntoElement + use<> {
    let step = values.clone();
    let label = match agent.state {
        AGENT_READY => "Remove",
        AGENT_SETUP => "Set up",
        _ => "Install",
    };
    h_flex()
        .justify_between()
        .items_center()
        .gap_4()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(h_flex().gap_2().items_center().child(strong(agent.name.clone())).child(pill(agent_state(agent.state))))
                .child(muted(agent.desc.clone())),
        )
        .child(Button::new(("agent", index)).label(label).small().on_click(move |_, _, cx| {
            let mut v = step.borrow_mut();
            let Some(a) = v.agents.get_mut(index) else { return };
            a.state = next_agent_state(a.state);
            cx.refresh_windows();
        }))
}

/// Install, then set up, then ready; removing goes back to not installed.
fn next_agent_state(state: u8) -> u8 {
    match state {
        AGENT_MISSING => AGENT_SETUP,
        AGENT_SETUP => AGENT_READY,
        _ => AGENT_MISSING,
    }
}

fn custom_group(values: &Shared) -> SettingGroup {
    let values = values.clone();
    SettingGroup::new()
        .title("Your own providers")
        .description("Any service that speaks a common API format.")
        .item(SettingItem::render(move |_options, _window, _cx| {
            let providers: Vec<CustomProvider> = values.borrow().custom_providers.clone();
            if providers.is_empty() {
                return muted("None added.").into_any_element();
            }
            let rows = providers.into_iter().map(|p| {
                let remove = values.clone();
                let id = p.id.clone();
                let models = p.models.iter().map(|m| m.id.clone()).collect::<Vec<_>>().join(", ");
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(h_flex().gap_2().items_center().child(strong(p.name.clone())).child(pill(p.format.clone())))
                            .child(mono(p.base_url.clone()))
                            .child(muted(models)),
                    )
                    .child(Button::new(SharedString::from(format!("custom-remove-{}", p.id))).label("Remove").small().ghost().on_click(
                        move |_, _, cx| {
                            remove.borrow_mut().custom_providers.retain(|c| c.id != id);
                            cx.refresh_windows();
                        },
                    ))
            });
            div().flex().flex_col().gap_3().children(rows).into_any_element()
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_keeps_only_its_ends() {
        assert_eq!(masked("sk-ant-api03-demo-5f2b"), "sk-a••••5f2b");
        assert_eq!(masked("short"), "••••");
    }

    #[test]
    fn agents_step_from_missing_to_ready_and_back() {
        assert_eq!(next_agent_state(AGENT_MISSING), AGENT_SETUP);
        assert_eq!(next_agent_state(AGENT_SETUP), AGENT_READY);
        assert_eq!(next_agent_state(AGENT_READY), AGENT_MISSING);
    }

    #[test]
    fn unknown_providers_show_their_id() {
        assert_eq!(provider_name("openai"), "OpenAI");
        assert_eq!(provider_name("zzz"), "zzz");
    }
}
