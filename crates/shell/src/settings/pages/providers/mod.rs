//! Providers: where models come from. API keys, the ChatGPT plan, cost-aware routing, coding agents
//! and providers the user defines.

mod agents;
mod custom;
mod keys;
mod routing;

use gpui_kit::base::h_flex;
use gpui_kit::component::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::setting::{SettingGroup, SettingItem, SettingPage};
use gpui_kit::prelude::*;
use gpui_kit::{App, SharedString, Styled, div};

use crate::backend;
use crate::settings::provider_state::CLOUD_PROVIDERS;
use crate::settings::values::Shared;
use crate::settings::widgets::{dot, muted, note, strong};
use crate::theme;

pub fn page(values: &Shared) -> SettingPage {
    start_loading(values);
    SettingPage::new("Providers")
        .icon(IconName::Globe)
        .description("Where the models come from, and the keys that unlock them.")
        .group(status_group(values))
        .group(chatgpt_group(values))
        .group(keys::group(values))
        .group(routing::group(values))
        .group(agents::group(values))
        .group(custom::group(values))
}

/// Reads the keys and settings the first time the page is drawn.
fn start_loading(values: &Shared) {
    {
        let v = values.borrow();
        if v.providers.loaded || v.providers.is_busy("load") {
            return;
        }
    }
    values.borrow_mut().providers.start("load");
    let values = values.clone();
    // The page is built without an `App`, so the read starts on the next draw that has one.
    LOAD_REQUEST.with(|slot| *slot.borrow_mut() = Some(values));
}

thread_local! {
    static LOAD_REQUEST: std::cell::RefCell<Option<Shared>> = const { std::cell::RefCell::new(None) };
}

/// Starts the read asked for by [`start_loading`]. Called from every group's render, which has an `App`.
fn run_pending_load(cx: &mut App) {
    let Some(values) = LOAD_REQUEST.with(|slot| slot.borrow_mut().take()) else { return };
    load(&values, cx);
}

/// Reads everything the page shows.
fn load(values: &Shared, cx: &mut App) {
    let Some(backend) = backend::get() else {
        let mut v = values.borrow_mut();
        v.providers.finish("load");
        v.providers.loaded = true;
        v.providers.error = Some("The key store could not be opened.".to_owned());
        return;
    };
    let hub = backend.hub.clone();
    let values = values.clone();
    backend::run(
        cx,
        async move {
            let custom = vivianne_ai::custom_providers::list().await;
            let mut ids: Vec<String> = CLOUD_PROVIDERS.iter().map(|(id, _)| (*id).to_owned()).collect();
            ids.extend(custom.iter().map(|p| p.id.clone()));
            let sources = hub.sources(&ids).await;
            let chatgpt = hub.chatgpt_status().await;
            let settings = vivianne_ai::host::read_settings_json().await;
            let routing = vivianne_ai::inference::routing::parse_config(settings.get("routing"));
            (custom, sources, chatgpt, routing)
        },
        move |(custom, sources, chatgpt, routing), cx| {
            {
                let mut v = values.borrow_mut();
                let p = &mut v.providers;
                p.sources = sources.into_iter().map(|(id, source)| (id, Some(source))).collect();
                p.custom_keyed = custom
                    .iter()
                    .filter(|c| p.connected(&c.id))
                    .map(|c| c.id.clone())
                    .collect();
                p.custom = custom;
                p.chatgpt = Some(chatgpt);
                p.routing = Some(routing);
                p.loaded = true;
                p.finish("load");
            }
            cx.refresh_windows();
        },
    );
}

/// A line telling what is wrong or still loading, above everything else.
fn status_group(values: &Shared) -> SettingGroup {
    let values = values.clone();
    SettingGroup::new().item(SettingItem::render(move |_options, _window, cx| {
        run_pending_load(cx);
        let v = values.borrow();
        if let Some(error) = &v.providers.error {
            return div().text_color(theme::color(theme::DANGER)).text_sm().child(error.clone()).into_any_element();
        }
        if !v.providers.loaded {
            return muted("Reading your keys and settings…").into_any_element();
        }
        muted("Keys are kept in your OS keychain, not in a plain file, when the computer has one.").into_any_element()
    }))
}

fn chatgpt_group(values: &Shared) -> SettingGroup {
    let values = values.clone();
    SettingGroup::new()
        .title("ChatGPT plan")
        .item(note("Use a ChatGPT Plus or Pro plan for OpenAI models instead of an API key. Usage counts against your plan, up to a weekly limit you set for Vivianne in ChatGPT."))
        .item(SettingItem::render(move |_options, _window, _cx| chatgpt_row(&values).into_any_element()))
}

fn chatgpt_row(values: &Shared) -> impl IntoElement + use<> {
    let (status, signing_in) = {
        let v = values.borrow();
        (v.providers.chatgpt.clone(), v.providers.is_busy("chatgpt"))
    };
    let connected = values.borrow().providers.chatgpt_connected();
    let signed_in_without_plan = status.as_ref().is_some_and(|s| s.signed_in && !s.sharing);
    let summary = match (&status, connected) {
        (Some(s), true) => format!("Using ChatGPT plan{}", s.email.as_ref().map(|e| format!(" · {e}")).unwrap_or_default()),
        _ if signed_in_without_plan => {
            "Signed in, but using your plan wasn't allowed during sign-in. Continue with ChatGPT again and allow plan usage.".to_owned()
        }
        _ => "Not signed in".to_owned(),
    };
    let (sign_in, sign_out, manage) = (values.clone(), values.clone(), values.clone());
    let mut buttons = h_flex().gap_2();
    if connected {
        buttons = buttons
            .child(Button::new("chatgpt-manage").label("Manage usage").on_click(move |_, _, cx| {
                if let Some(backend) = backend::get()
                    && let Err(why) = backend.hub.chatgpt_manage_usage()
                {
                    manage.borrow_mut().providers.error = Some(why);
                    cx.refresh_windows();
                }
            }))
            .child(Button::new("chatgpt-out").label("Sign out").ghost().on_click(move |_, _, cx| {
                keys::run_chatgpt(&sign_out, cx, ChatGptCall::SignOut);
            }));
    } else {
        let label = if signing_in { "Finish signing in in your browser…" } else { "Continue with ChatGPT" };
        buttons = buttons.child(Button::new("chatgpt-in").label(label).on_click(move |_, _, cx| {
            if !sign_in.borrow().providers.is_busy("chatgpt") {
                keys::run_chatgpt(&sign_in, cx, ChatGptCall::SignIn);
            }
        }));
    }
    h_flex()
        .justify_between()
        .items_center()
        .gap_4()
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(dot(if connected { theme::OK } else { theme::MUTED }))
                .child(strong(SharedString::from(summary))),
        )
        .child(buttons)
}

/// What the ChatGPT buttons ask for.
#[derive(Clone, Copy)]
pub enum ChatGptCall {
    SignIn,
    SignOut,
}
