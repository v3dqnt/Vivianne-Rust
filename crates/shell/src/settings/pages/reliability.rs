//! Reliability: retrying a failed request, fallback models, and the limits on a run.

use gpui_kit::base::h_flex;
use gpui_kit::component::IconName;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::setting::{NumberFieldOptions, SettingField, SettingGroup, SettingItem, SettingPage};
use gpui_kit::prelude::*;
use gpui_kit::{Styled, div};

use crate::settings::values::Shared;
use crate::settings::widgets::{btn, field_switch, keyed_input, muted, note, strong};

const ADD_FALLBACK: &str = "fallback-add";

pub fn page(values: &Shared) -> SettingPage {
    SettingPage::new("Reliability")
        .icon(IconName::Heart)
        .description("What happens when a model fails, and how far one run may go.")
        .group(retry_group(values))
        .group(fallback_group(values))
        .group(limits_group(values))
}

fn retry_group(values: &Shared) -> SettingGroup {
    SettingGroup::new().item(
        SettingItem::new("Retry temporary failures", field_switch(values, |v| v.retry_transient, |v, on| v.retry_transient = on))
            .description("Try again, with a short wait, after a timeout or an overloaded provider."),
    )
}

fn fallback_group(values: &Shared) -> SettingGroup {
    let list = values.clone();
    let add = values.clone();
    SettingGroup::new()
        .title("Fallback models")
        .description("If the selected model keeps failing, these are tried in order.")
        .item(SettingItem::render(move |_options, _window, _cx| {
            let entries = list.borrow().fallbacks.clone();
            if entries.is_empty() {
                return muted("No fallbacks. A failing model just fails.").into_any_element();
            }
            let rows = entries.into_iter().enumerate().map(|(index, entry)| {
                let values = list.clone();
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(strong(format!("{}. {}", index + 1, entry.label)))
                    .child(
                        Button::new(("fallback-remove", index))
                            .label("Remove")
                            .ghost()
                            .small()
                            .on_click(move |_, _, cx| {
                                values.borrow_mut().fallbacks.remove(index);
                                cx.refresh_windows();
                            }),
                    )
            });
            div().flex().flex_col().gap_2().children(rows).into_any_element()
        }))
        .item(SettingItem::render(move |_options, window, cx| {
            let values = add.clone();
            let input = keyed_input(
                ADD_FALLBACK,
                &values,
                |v| v.key_drafts.get(ADD_FALLBACK).cloned().unwrap_or_default(),
                |v, x| {
                    v.key_drafts.insert(ADD_FALLBACK.to_owned(), x);
                },
                "provider/model, e.g. openai/gpt-5-nano",
                window,
                cx,
            );
            h_flex()
                .gap_2()
                .child(input)
                .child(btn("fallback-add-button", "Add", move |_, _, cx| {
                    let mut v = values.borrow_mut();
                    let draft = v.key_drafts.remove(ADD_FALLBACK).unwrap_or_default();
                    let draft = draft.trim().to_owned();
                    if !draft.is_empty() && !v.fallbacks.iter().any(|f| f.id == draft) {
                        v.fallbacks.push(crate::settings::values::FallbackModel { id: draft.clone(), label: draft });
                    }
                    cx.refresh_windows();
                }))
                .into_any_element()
        }))
}

fn limits_group(values: &Shared) -> SettingGroup {
    let count = values.borrow().limits.len();
    let items = (0..count).map(|index| limit_item(values, index));
    SettingGroup::new()
        .title("Limits")
        .items(std::iter::once(note("Raise these with care: more runs more of your plan and your machine.")).chain(items))
}

fn limit_item(values: &Shared, index: usize) -> SettingItem {
    let (label, unit, min, max, default, hint) = {
        let v = values.borrow();
        let l = &v.limits[index];
        (l.label.clone(), l.unit.clone(), l.min, l.max, l.default, l.hint.clone())
    };
    let (read, write) = (values.clone(), values.clone());
    let field = SettingField::number_input(
        NumberFieldOptions { min, max, step: 1.0 },
        move |_| read.borrow().limits[index].value,
        move |value, cx| {
            write.borrow_mut().limits[index].value = value;
            cx.refresh_windows();
        },
    )
    .default_value(default);
    let description = if hint.is_empty() { format!("In {unit}. Default {default}.") } else { format!("{hint} In {unit}. Default {default}.") };
    SettingItem::new(label, field).description(description)
}
