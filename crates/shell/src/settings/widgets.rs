//! Shared element and field helpers for the Settings pages. Every setter
//! calls `cx.refresh_windows()`: Kit's Switch, Dropdown, Input and NumberField
//! renderers never notify on their own, so nothing updates without it.

use std::rc::Rc;

use gpui_kit::base::input::{Textarea, TextareaState};
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::setting::{NumberFieldOptions, SettingField, SettingItem};
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyView, App, AppContext as _, ClickEvent, ClipboardItem, Context, Entity, IntoElement,
    SharedString, Styled, Subscription, Window, div, prelude::FluentBuilder as _, px,
};

use super::values::{Shared, Values};
use crate::theme::{self, color};

// ---------------------------------------------------------------- items ---

/// A line of plain muted text, as its own item in a group.
pub fn note(text: impl Into<SharedString> + 'static) -> SettingItem {
    let text = text.into();
    SettingItem::render(move |_options, _window, _cx| {
        div()
            .text_color(color(theme::MUTED))
            .text_sm()
            .child(text.clone())
            .into_any_element()
    })
}

/// A bold small heading, as its own item in a group.
pub fn heading(text: impl Into<SharedString> + 'static) -> SettingItem {
    let text = text.into();
    SettingItem::render(move |_options, _window, _cx| {
        div()
            .text_color(color(theme::TEXT))
            .text_sm()
            .font_weight(gpui_kit::FontWeight::MEDIUM)
            .child(text.clone())
            .into_any_element()
    })
}

/// Muted small text to sit under a heading inside a custom element.
pub fn muted(text: impl Into<SharedString> + 'static) -> impl IntoElement {
    div()
        .text_color(color(theme::MUTED))
        .text_sm()
        .child(text.into())
}

/// Normal text color, small.
pub fn strong(text: impl Into<SharedString> + 'static) -> impl IntoElement {
    div()
        .text_color(color(theme::TEXT))
        .text_sm()
        .child(text.into())
}

/// Monospaced small text (paths, flags, key ids).
pub fn mono(text: impl Into<SharedString> + 'static) -> impl IntoElement {
    div()
        .font_family(theme::MONO)
        .text_sm()
        .text_color(color(theme::TEXT))
        .child(text.into())
}

/// A small rounded chip with muted text (states, counts, "BETA").
pub fn pill(text: impl Into<SharedString> + 'static) -> impl IntoElement {
    div()
        .px_2()
        .py_0p5()
        .rounded_full()
        .border_1()
        .border_color(color(theme::BORDER))
        .bg(color(theme::SURFACE))
        .text_color(color(theme::MUTED))
        .text_xs()
        .child(text.into())
}

/// A small status dot in the given color.
pub fn dot(hex: u32) -> impl IntoElement {
    div().size(px(8.0)).rounded_full().bg(color(hex))
}

/// A number and its label, as the stat tiles use.
pub fn stat(value: impl Into<SharedString> + 'static, label: &'static str) -> impl IntoElement {
    v_flex()
        .gap_0p5()
        .child(
            div()
                .text_color(color(theme::TEXT))
                .text_xl()
                .child(value.into()),
        )
        .child(div().text_color(color(theme::MUTED)).text_xs().child(label))
}

/// A horizontal row with the standard gap, for stacking inline widgets.
pub fn row(children: impl IntoIterator<Item = impl IntoElement>) -> impl IntoElement {
    h_flex().gap_2().items_center().children(children)
}

// ---------------------------------------------------------------- fields ---

/// A Switch bound to a bool in `Values`.
pub fn field_switch(
    values: &Shared,
    get: fn(&Values) -> bool,
    set: fn(&mut Values, bool),
) -> SettingField<bool> {
    let (read, write) = (values.clone(), values.clone());
    SettingField::switch(
        move |_| get(&read.borrow()),
        move |on, cx| {
            set(&mut write.borrow_mut(), on);
            cx.refresh_windows();
        },
    )
}

/// A Dropdown bound to a string in `Values`. `options` pairs values with labels.
pub fn field_dropdown(
    values: &Shared,
    options: &[(&'static str, &'static str)],
    get: fn(&Values) -> String,
    set: fn(&mut Values, String),
) -> SettingField<SharedString> {
    let options = options
        .iter()
        .map(|(value, label)| (SharedString::from(*value), SharedString::from(*label)))
        .collect();
    let (read, write) = (values.clone(), values.clone());
    SettingField::dropdown(
        options,
        move |_| SharedString::from(get(&read.borrow())),
        move |value, cx| {
            set(&mut write.borrow_mut(), value.to_string());
            cx.refresh_windows();
        },
    )
}

/// A single-line input bound to a string in `Values`.
pub fn field_input(
    values: &Shared,
    get: fn(&Values) -> String,
    set: fn(&mut Values, String),
) -> SettingField<SharedString> {
    let (read, write) = (values.clone(), values.clone());
    SettingField::input(
        move |_| SharedString::from(get(&read.borrow())),
        move |value, cx| {
            set(&mut write.borrow_mut(), value.to_string());
            cx.refresh_windows();
        },
    )
}

/// A number input bound to an f64 in `Values`.
pub fn field_number(
    values: &Shared,
    options: NumberFieldOptions,
    get: fn(&Values) -> f64,
    set: fn(&mut Values, f64),
) -> SettingField<f64> {
    let (read, write) = (values.clone(), values.clone());
    SettingField::number_input(
        options,
        move |_| get(&read.borrow()),
        move |value, cx| {
            set(&mut write.borrow_mut(), value);
            cx.refresh_windows();
        },
    )
}

// -------------------------------------------------------------- widgets ---

/// Persistent state for [`keyed_input`]: the input plus its subscription,
/// kept alive for as long as the keyed state exists.
struct KeyedInput {
    input: Entity<InputState>,
    _subscription: Subscription,
}

/// Persistent state for [`keyed_textarea`]: same shape, multi-line mode.
struct KeyedTextarea {
    input: Entity<TextareaState>,
    _subscription: Subscription,
}

#[allow(clippy::type_complexity)]
fn keyed_state(
    key: &'static str,
    values: &Shared,
    set: fn(&mut Values, String),
    value: String,
    placeholder: &'static str,
    window: &mut Window,
    cx: &mut App,
) -> Entity<KeyedInput> {
    window.use_keyed_state(SharedString::from(key), cx, {
        let value = value.clone();
        move |window: &mut Window, cx: &mut Context<KeyedInput>| {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(value.clone())
                    .placeholder(placeholder)
            });
            let values = values.clone();
            let subscription = cx.subscribe(&input, move |_, input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    let typed = input.read(cx).value().to_string();
                    set(&mut values.borrow_mut(), typed);
                    cx.refresh_windows();
                }
            });
            KeyedInput {
                input,
                _subscription: subscription,
            }
        }
    })
}

/// The textarea twin of [`keyed_state`]: a multi-line input that grows
/// between `rows.0` and `rows.1` lines, bound to a string in `Values`.
#[allow(clippy::type_complexity)]
fn keyed_textarea_state(
    key: &'static str,
    values: &Shared,
    set: fn(&mut Values, String),
    value: String,
    placeholder: &'static str,
    rows: (u8, u8),
    window: &mut Window,
    cx: &mut App,
) -> Entity<KeyedTextarea> {
    window.use_keyed_state(SharedString::from(key), cx, {
        let value = value.clone();
        move |window: &mut Window, cx: &mut Context<KeyedTextarea>| {
            let input = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .default_value(value.clone())
                    .placeholder(placeholder)
                    .auto_grow(usize::from(rows.0), usize::from(rows.1))
            });
            let values = values.clone();
            let subscription = cx.subscribe(&input, move |_, input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    let typed = input.read(cx).value().to_string();
                    set(&mut values.borrow_mut(), typed);
                    cx.refresh_windows();
                }
            });
            KeyedTextarea {
                input,
                _subscription: subscription,
            }
        }
    })
}

/// An inline single-line input bound to a string in `Values`. Used inside
/// custom render closures; mirrors Kit's own `StringField`, including syncing
/// the displayed text when the value changes from elsewhere (a dialog save).
pub fn keyed_input(
    key: &'static str,
    values: &Shared,
    get: fn(&Values) -> String,
    set: fn(&mut Values, String),
    placeholder: &'static str,
    window: &mut Window,
    cx: &mut App,
) -> Input {
    let value = get(&values.borrow());
    let state = keyed_state(key, values, set, value.clone(), placeholder, window, cx);
    sync_input(&state, &value, window, cx);
    Input::new(&state.read(cx).input).w_full()
}

/// An inline multi-line textarea bound to a string in `Values`, growing
/// between `rows.0` and `rows.1` lines.
pub fn keyed_textarea(
    key: &'static str,
    values: &Shared,
    get: fn(&Values) -> String,
    set: fn(&mut Values, String),
    placeholder: &'static str,
    rows: (u8, u8),
    window: &mut Window,
    cx: &mut App,
) -> Textarea {
    let value = get(&values.borrow());
    let state = keyed_textarea_state(
        key,
        values,
        set,
        value.clone(),
        placeholder,
        rows,
        window,
        cx,
    );
    sync_textarea(&state, &value, window, cx);
    Textarea::new(&state.read(cx).input)
}

/// Re-display `value` on an input whose text drifted (external change).
fn sync_input(state: &Entity<KeyedInput>, value: &str, window: &mut Window, cx: &mut App) {
    let input = state.read(cx).input.clone();
    if input.read(cx).value() != value {
        input.update(cx, |input, cx| {
            input.set_value(value.to_string(), window, cx);
        });
    }
}

/// The [`sync_input`] twin for textareas.
fn sync_textarea(state: &Entity<KeyedTextarea>, value: &str, window: &mut Window, cx: &mut App) {
    let input = state.read(cx).input.clone();
    if input.read(cx).value() != value {
        input.update(cx, |input, cx| {
            input.set_value(value.to_string(), window, cx);
        });
    }
}

/// An inline Switch bound to a bool in `Values`, for custom render closures.
pub fn toggle(
    key: &'static str,
    values: &Shared,
    get: fn(&Values) -> bool,
    set: fn(&mut Values, bool),
) -> Switch {
    let (read, write) = (values.clone(), values.clone());
    Switch::new(key)
        .checked(get(&read.borrow()))
        .on_click(move |on, _window, cx| {
            set(&mut write.borrow_mut(), *on);
            cx.refresh_windows();
        })
}

/// A default-styled button. Chain `.primary()`, `.secondary()`, `.danger()`
/// and friends for other looks.
pub fn btn(
    key: &'static str,
    label: &'static str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Button {
    Button::new(key).label(label).on_click(on_click)
}

/// A ghost button that copies a URL to the clipboard, for links shown as text.
pub fn link_btn(key: &'static str, label: &'static str, url: &'static str) -> Button {
    Button::new(key)
        .label(label)
        .ghost()
        .on_click(move |_, _, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(url.into()));
        })
}

// -------------------------------------------------------------- tooltips ---

/// A small tooltip popup view: dark card with a border and small text.
struct TipView {
    text: SharedString,
}

impl Render for TipView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .max_w(px(280.0))
            .px_2()
            .py_1()
            .rounded(px(6.0))
            .bg(color(theme::BG))
            .border_1()
            .border_color(color(theme::BORDER))
            .text_xs()
            .text_color(color(theme::TEXT))
            .child(self.text.clone())
    }
}

/// Build a tooltip view for `hoverable_tooltip` on a heatmap cell or similar:
/// `.hoverable_tooltip(move |_, cx| widgets::hover_tip(cx, "…"))`.
pub fn hover_tip(cx: &mut App, text: impl Into<SharedString>) -> AnyView {
    cx.new(|_| TipView { text: text.into() }).into()
}

// -------------------------------------------------------------- dialogs ---

/// Open a confirmation alert. The OK button runs `on_ok` and closes the
/// dialog; the Cancel button only closes it.
pub fn confirm(
    window: &mut Window,
    cx: &mut App,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    ok_label: impl Into<SharedString>,
    danger: bool,
    on_ok: impl Fn(&mut Window, &mut App) + 'static,
) {
    let on_ok = Rc::new(on_ok);
    let (title, description, ok_label) = (title.into(), description.into(), ok_label.into());
    window.open_alert_dialog(cx, move |alert, _window, _cx| {
        alert
            .title(title.clone())
            .description(description.clone())
            .confirm()
            .ok_text(ok_label.clone())
            .when(danger, |alert| alert.ok_variant(ButtonVariant::Danger))
            .on_ok({
                let on_ok = on_ok.clone();
                move |_, window, cx| {
                    on_ok(window, cx);
                    true
                }
            })
    });
}
