//! Providers the user defines: any server that speaks one of the four wire formats.

use gpui_kit::base::h_flex;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::setting::{SettingGroup, SettingItem};
use gpui_kit::prelude::*;
use gpui_kit::{App, SharedString, Styled, Window, div};
use vivianne_ai::custom_providers::{self, ApiFormat, CustomModel, CustomProvider};

use super::keys::reload_sources;
use crate::backend;
use crate::settings::provider_state::Pane;
use crate::settings::values::Shared;
use crate::settings::widgets::{btn, dot, keyed_input, keyed_secret, mono, muted, note, strong};
use crate::theme;

const NAME: &str = "custom-name";
const URL: &str = "custom-url";
const KEY: &str = "custom-key";
const MODEL: &str = "custom-model";
const CONTEXT: &str = "custom-context";
const DEFAULT_CONTEXT: u64 = 200_000;

pub fn group(values: &Shared) -> SettingGroup {
    let values = values.clone();
    SettingGroup::new()
        .title("Add a custom provider")
        .item(note("Any service that speaks one of the common API formats. Models are entered by hand, so adding one needs no network."))
        .item(SettingItem::render(move |_options, window, cx| view(&values, window, cx).into_any_element()))
}

fn view(values: &Shared, window: &mut Window, cx: &mut App) -> impl IntoElement + use<> {
    let (providers, pane, keyed) = {
        let v = values.borrow();
        (
            v.providers.custom.clone(),
            v.providers.pane.clone(),
            v.providers.custom_keyed.clone(),
        )
    };
    let rows: Vec<_> = providers
        .iter()
        .map(|p| {
            row(
                values,
                p,
                keyed.contains(&p.id),
                pane == Pane::Custom(p.id.clone()),
            )
        })
        .collect();
    let add = values.clone();
    let adding = pane == Pane::NewCustom;
    let add_button = Button::new("custom-add")
        .label(if adding { "Cancel" } else { "Add provider" })
        .small()
        .on_click(move |_, _, cx| {
            if adding {
                add.borrow_mut().providers.pane = Pane::Nothing;
            } else {
                begin_new(&add);
            }
            cx.refresh_windows();
        });
    let form = match &pane {
        Pane::NewCustom => Some(form(values, None, window, cx)),
        Pane::Custom(id) => providers
            .iter()
            .find(|p| p.id == *id)
            .map(|p| form(values, Some(p.clone()), window, cx)),
        _ => None,
    };
    div()
        .flex()
        .flex_col()
        .gap_2()
        .children(rows)
        .child(add_button)
        .children(form)
}

fn row(
    values: &Shared,
    provider: &CustomProvider,
    has_key: bool,
    open: bool,
) -> impl IntoElement + use<> {
    let (toggle, id) = (values.clone(), provider.id.clone());
    h_flex()
        .justify_between()
        .items_center()
        .gap_4()
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(dot(if has_key { theme::OK } else { theme::MUTED }))
                .child(strong(provider.name.clone()))
                .child(muted(provider.api_format.label())),
        )
        .child(
            Button::new(SharedString::from(format!("custom-{}", provider.id)))
                .label(if open { "Close" } else { "Edit" })
                .small()
                .ghost()
                .on_click(move |_, _, cx| {
                    if open {
                        toggle.borrow_mut().providers.pane = Pane::Nothing;
                    } else {
                        begin_edit(&toggle, &id);
                    }
                    cx.refresh_windows();
                }),
        )
}

fn begin_new(values: &Shared) {
    let mut v = values.borrow_mut();
    let p = &mut v.providers;
    p.pane = Pane::NewCustom;
    p.new_format = Some(ApiFormat::OpenAiChat);
    p.new_models.clear();
    for name in [NAME, URL, KEY, MODEL, CONTEXT] {
        p.clear_draft(name);
    }
    p.error = None;
}

fn begin_edit(values: &Shared, id: &str) {
    let mut v = values.borrow_mut();
    let Some(found) = v.providers.custom.iter().find(|p| p.id == id).cloned() else {
        return;
    };
    let p = &mut v.providers;
    p.pane = Pane::Custom(id.to_owned());
    p.set_draft(NAME, found.name);
    p.set_draft(URL, found.base_url);
    for name in [KEY, MODEL, CONTEXT] {
        p.clear_draft(name);
    }
    p.error = None;
}

fn form(
    values: &Shared,
    existing: Option<CustomProvider>,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement + use<> {
    let name = keyed_input(
        NAME,
        values,
        |v| v.providers.draft(NAME),
        |v, x| v.providers.set_draft(NAME, x),
        "Name, for example DeepSeek",
        window,
        cx,
    );
    let url = keyed_input(
        URL,
        values,
        |v| v.providers.draft(URL),
        |v, x| v.providers.set_draft(URL, x),
        "Base URL, for example https://api.example.com/v1",
        window,
        cx,
    );
    let key_hint = if existing.is_some() {
        "Key set; leave blank to keep it"
    } else {
        "API key"
    };
    let key = keyed_secret(
        KEY,
        values,
        |v| v.providers.draft(KEY),
        |v, x| v.providers.set_draft(KEY, x),
        key_hint,
        window,
        cx,
    );
    let model = keyed_input(
        MODEL,
        values,
        |v| v.providers.draft(MODEL),
        |v, x| v.providers.set_draft(MODEL, x),
        "Model id",
        window,
        cx,
    );
    let context = keyed_input(
        CONTEXT,
        values,
        |v| v.providers.draft(CONTEXT),
        |v, x| v.providers.set_draft(CONTEXT, x),
        "Context window (tokens), default 200000",
        window,
        cx,
    );

    let models: Vec<(String, u64)> = match &existing {
        Some(p) => p
            .models
            .iter()
            .map(|m| (m.id.clone(), m.context_window))
            .collect(),
        None => values.borrow().providers.new_models.clone(),
    };
    let model_rows: Vec<_> = models
        .iter()
        .map(|(id, ctx)| model_row(values, existing.as_ref(), id, *ctx))
        .collect();
    let format = format_picker(values, existing.as_ref());

    let (add_model, save, remove) = (values.clone(), values.clone(), values.clone());
    let (existing_for_model, existing_for_save) = (existing.clone(), existing.clone());
    let mut buttons = h_flex().gap_2().child(btn(
        "custom-save",
        if existing.is_some() {
            "Save provider"
        } else {
            "Add provider"
        },
        move |_, _, cx| {
            save_provider(&save, existing_for_save.clone(), cx);
        },
    ));
    if let Some(p) = &existing {
        let id = p.id.clone();
        buttons = buttons.child(
            Button::new("custom-remove")
                .label("Remove")
                .ghost()
                .on_click(move |_, _, cx| remove_provider(&remove, &id, cx)),
        );
    }
    div()
        .flex()
        .flex_col()
        .gap_2()
        .p_3()
        .rounded_md()
        .border_1()
        .border_color(theme::color(theme::BORDER))
        .child(strong(if existing.is_some() {
            "Edit provider"
        } else {
            "Add provider"
        }))
        .child(name)
        .child(url)
        .child(key)
        .child(format)
        .child(strong("Models"))
        .children(if model_rows.is_empty() {
            vec![muted("No models added yet.").into_any_element()]
        } else {
            model_rows
                .into_iter()
                .map(IntoElement::into_any_element)
                .collect()
        })
        .child(model)
        .child(context)
        .child(btn("custom-add-model", "Add model", move |_, _, cx| {
            add_model_to(&add_model, existing_for_model.clone(), cx)
        }))
        .child(buttons)
}

/// The format is fixed once a provider exists: its id carries it.
fn format_picker(values: &Shared, existing: Option<&CustomProvider>) -> impl IntoElement + use<> {
    if let Some(p) = existing {
        return muted(format!(
            "Format: {} (fixed once created; remove and re-add to change it)",
            p.api_format.label()
        ))
        .into_any_element();
    }
    let current = values.borrow().providers.new_format;
    let buttons = ApiFormat::ALL.map(|format| {
        let values = values.clone();
        let mut button = Button::new(SharedString::from(format!("format-{}", format.name())))
            .label(format.label())
            .small();
        if current != Some(format) {
            button = button.ghost();
        }
        button.on_click(move |_, _, cx| {
            values.borrow_mut().providers.new_format = Some(format);
            cx.refresh_windows();
        })
    });
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(muted("API format"))
        .children(buttons)
        .into_any_element()
}

fn model_row(
    values: &Shared,
    existing: Option<&CustomProvider>,
    id: &str,
    context: u64,
) -> impl IntoElement + use<> {
    let (values, existing, model) = (values.clone(), existing.cloned(), id.to_owned());
    h_flex()
        .justify_between()
        .items_center()
        .child(
            h_flex()
                .gap_3()
                .items_center()
                .child(mono(id.to_owned()))
                .child(muted(format!("{}K context", context / 1000))),
        )
        .child(
            Button::new(SharedString::from(format!("custom-model-remove-{id}")))
                .label("Remove")
                .small()
                .ghost()
                .on_click(move |_, _, cx| match &existing {
                    Some(provider) => {
                        let mut edited = provider.clone();
                        edited.models.retain(|m| m.id != model);
                        store(&values, edited, cx);
                    }
                    None => {
                        values
                            .borrow_mut()
                            .providers
                            .new_models
                            .retain(|(m, _)| *m != model);
                        cx.refresh_windows();
                    }
                }),
        )
}

/// Takes the model typed in the form: onto the provider if it exists, otherwise onto the new one.
fn add_model_to(values: &Shared, existing: Option<CustomProvider>, cx: &mut App) {
    let (id, context) = {
        let v = values.borrow();
        (
            v.providers.draft(MODEL).trim().to_owned(),
            v.providers.draft(CONTEXT),
        )
    };
    if id.is_empty() {
        return;
    }
    let context = context
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_CONTEXT);
    {
        let mut v = values.borrow_mut();
        v.providers.clear_draft(MODEL);
        v.providers.clear_draft(CONTEXT);
    }
    match existing {
        Some(mut provider) => {
            provider.models.retain(|m| m.id != id);
            provider.models.push(CustomModel {
                id,
                context_window: context,
                max_output_tokens: None,
            });
            store(values, provider, cx);
        }
        None => {
            let mut v = values.borrow_mut();
            v.providers.new_models.retain(|(m, _)| *m != id);
            v.providers.new_models.push((id, context));
            drop(v);
            cx.refresh_windows();
        }
    }
}

/// Saves the provider and shows the list as it now is.
fn store(values: &Shared, provider: CustomProvider, cx: &mut App) {
    let values = values.clone();
    let saved = provider.clone();
    backend::run(
        cx,
        async move { custom_providers::save(provider).await },
        move |result, cx| {
            let mut v = values.borrow_mut();
            match result {
                Ok(()) => match v.providers.custom.iter_mut().find(|p| p.id == saved.id) {
                    Some(existing) => *existing = saved,
                    None => v.providers.custom.push(saved),
                },
                Err(why) => v.providers.error = Some(why),
            }
            drop(v);
            cx.refresh_windows();
        },
    );
}

fn save_provider(values: &Shared, existing: Option<CustomProvider>, cx: &mut App) {
    let (name, url, key, format, models) = {
        let v = values.borrow();
        let p = &v.providers;
        (
            p.draft(NAME).trim().to_owned(),
            p.draft(URL).trim().to_owned(),
            p.draft(KEY),
            p.new_format,
            p.new_models.clone(),
        )
    };
    if name.is_empty() || url.is_empty() {
        values.borrow_mut().providers.error =
            Some("A provider needs a name and a base URL.".to_owned());
        cx.refresh_windows();
        return;
    }
    let provider = match existing {
        Some(mut p) => {
            p.name = name;
            p.base_url = url;
            p
        }
        None => {
            let format = format.unwrap_or(ApiFormat::OpenAiChat);
            let models = models
                .into_iter()
                .map(|(id, context_window)| CustomModel {
                    id,
                    context_window,
                    max_output_tokens: None,
                })
                .collect();
            CustomProvider {
                id: custom_providers::new_id(format),
                name,
                base_url: url,
                api_format: format,
                models,
            }
        }
    };
    let Some(hub) = backend::get().map(|b| b.hub.clone()) else {
        return;
    };
    let (values, id, saved) = (values.clone(), provider.id.clone(), provider.clone());
    backend::run(
        cx,
        async move {
            if !key.trim().is_empty() {
                hub.save_key(&id, &key).await?;
            }
            custom_providers::save(provider).await
        },
        move |result, cx| {
            {
                let mut v = values.borrow_mut();
                match result {
                    Ok(()) => {
                        match v.providers.custom.iter_mut().find(|p| p.id == saved.id) {
                            Some(existing) => *existing = saved,
                            None => v.providers.custom.push(saved),
                        }
                        v.providers.pane = Pane::Nothing;
                        v.providers.clear_draft(KEY);
                    }
                    Err(why) => v.providers.error = Some(why),
                }
            }
            reload_sources(&values, cx);
            cx.refresh_windows();
        },
    );
}

fn remove_provider(values: &Shared, id: &str, cx: &mut App) {
    let Some(hub) = backend::get().map(|b| b.hub.clone()) else {
        return;
    };
    let (values, id) = (values.clone(), id.to_owned());
    let ask = id.clone();
    backend::run(
        cx,
        async move {
            // A provider with no key saved has nothing to delete there.
            let _ = hub.delete_key(&ask).await;
            custom_providers::remove(&ask).await
        },
        move |result, cx| {
            {
                let mut v = values.borrow_mut();
                match result {
                    Ok(()) => {
                        v.providers.custom.retain(|p| p.id != id);
                        v.providers.pane = Pane::Nothing;
                    }
                    Err(why) => v.providers.error = Some(why),
                }
            }
            reload_sources(&values, cx);
            cx.refresh_windows();
        },
    );
}
