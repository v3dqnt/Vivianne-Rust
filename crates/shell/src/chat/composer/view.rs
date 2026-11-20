//! Draws the composer card. All state lives on `ChatView` / `Composer`.

use gpui_kit::base::Disableable as _;
use gpui_kit::component::attachment::{
    Attachment, AttachmentContent, AttachmentDescription, AttachmentGroup, AttachmentMedia,
    AttachmentTitle,
};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::combobox::Combobox;
use gpui_kit::component::input::Textarea;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Icon, IconName, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{Anchor, Context, ExternalPaths, Focusable as _, Hsla, Window, div, px, rgb};

use super::{Mode, Thinking, thinking_label};
use super::attachments::size_label;
use crate::chat::{ChatView, shorten};
use crate::settings;
use crate::theme::{self, color};

/// Longest model name shown on the toolbar before it is cut.
const MODEL_LABEL_CHARS: usize = 24;
const MODEL_MENU_WIDTH: f32 = 380.0;
const MODEL_MENU_HEIGHT: f32 = 320.0;
const SEND_SIZE: f32 = 28.0;

pub fn render(
    chat: &ChatView,
    window: &mut Window,
    cx: &mut Context<ChatView>,
) -> impl IntoElement {
    let composer = &chat.composer;
    let input = composer.input.read(cx);
    let focused = input.focus_handle(cx).is_focused(window);
    let can_send = !input.value().trim().is_empty() || !composer.attachments.is_empty();

    div()
        .px(px(theme::SIDE_PAD))
        .pb(px(10.0))
        .flex()
        .justify_center()
        .child(
            div()
                .id("composer")
                .w_full()
                .max_w(px(theme::COLUMN_MAX))
                .flex()
                .flex_col()
                .gap(px(6.0))
                .px(px(12.0))
                .pt(px(10.0))
                .pb(px(8.0))
                .rounded(px(theme::RADIUS_COMPOSER))
                .border_1()
                .border_color(color(if focused {
                    theme::ACCENT
                } else {
                    theme::BORDER
                }))
                .bg(color(theme::SURFACE))
                .shadow_lg()
                .drag_over::<ExternalPaths>(|style, _, _, _| {
                    style.border_color(color(theme::ACCENT))
                })
                .on_drop(cx.listener(|chat, paths: &ExternalPaths, _window, cx| {
                    chat.attach_paths(paths.paths(), cx);
                }))
                .when(!composer.attachments.is_empty(), |card| {
                    card.child(chips(chat, cx))
                })
                .child(Textarea::new(&composer.input).appearance(false))
                .child(toolbar(chat, can_send, cx)),
        )
}

fn chips(chat: &ChatView, cx: &mut Context<ChatView>) -> impl IntoElement {
    AttachmentGroup::new("attachment-chips").children(
        chat.composer
            .attachments
            .iter()
            .enumerate()
            .map(|(index, file)| {
                Attachment::new()
                    .id(("attachment", index))
                    .small()
                    .media(AttachmentMedia::new().child(Icon::new(IconName::FileText)))
                    .content(
                        AttachmentContent::new()
                            .title(AttachmentTitle::new(file.name.clone()))
                            .description(AttachmentDescription::new(size_label(file.size))),
                    )
                    .on_remove(cx.listener(move |chat, _event, _window, cx| {
                        chat.remove_attachment(index, cx);
                    }))
            }),
    )
}

fn toolbar(chat: &ChatView, can_send: bool, cx: &mut Context<ChatView>) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(2.0))
                .child(
                    Button::new("attach")
                        .ghost()
                        .small()
                        .icon(IconName::Plus)
                        .tooltip("Attach files")
                        .on_click(
                            cx.listener(|_chat, _event, _window, cx| ChatView::pick_files(cx)),
                        ),
                )
                .child(mode_menu(chat, cx))
                .child(thinking_menu(chat, cx))
                .child(model_menu(chat)),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(4.0))
                .child(
                    Button::new("voice")
                        .ghost()
                        .small()
                        .icon(IconName::Mic)
                        .tooltip("Voice input is not available yet")
                        .disabled(true),
                )
                .child(send_button(chat.running, can_send, cx)),
        )
}

fn mode_menu(chat: &ChatView, cx: &mut Context<ChatView>) -> impl IntoElement {
    let me = cx.entity().downgrade();
    let current = chat.composer.mode;
    Button::new("mode")
        .ghost()
        .small()
        .label(current.label())
        .dropdown_caret(true)
        .tooltip(current.hint())
        .dropdown_menu_with_anchor(Anchor::BottomLeft, move |menu, _window, _cx| {
            Mode::ALL.iter().fold(menu, |menu, &mode| {
                let me = me.clone();
                menu.item(
                    PopupMenuItem::new(format!("{}: {}", mode.label(), mode.hint()))
                        .checked(mode == current)
                        .on_click(move |_event, _window, app| {
                            me.update(app, |chat, cx| chat.set_mode(mode, cx)).ok();
                        }),
                )
            })
        })
}

fn thinking_menu(chat: &ChatView, cx: &mut Context<ChatView>) -> impl IntoElement {
    let me = cx.entity().downgrade();
    let current = chat.composer.thinking;
    Button::new("thinking")
        .ghost()
        .small()
        .label(format!("Thinking: {}", thinking_label(current)))
        .dropdown_caret(true)
        .tooltip("How much the model thinks before answering")
        .dropdown_menu_with_anchor(Anchor::BottomLeft, move |menu, _window, _cx| {
            Thinking::ALL.iter().fold(menu, |menu, &level| {
                let me = me.clone();
                menu.item(
                    PopupMenuItem::new(thinking_label(level))
                        .checked(level == current)
                        .on_click(move |_event, _window, app| {
                            me.update(app, |chat, cx| chat.set_thinking(level, cx)).ok();
                        }),
                )
            })
        })
}

fn model_menu(chat: &ChatView) -> impl IntoElement {
    let label = shorten(&chat.selection.model, MODEL_LABEL_CHARS);
    Combobox::new(&chat.picker)
        .placeholder(label.clone())
        .search_placeholder("Search models")
        .menu_width(px(MODEL_MENU_WIDTH))
        .menu_max_h(px(MODEL_MENU_HEIGHT))
        .appearance(false)
        .empty(|_window, _cx| div().p_3().text_sm().child("No models found. Save a provider key in Settings."))
        .render_trigger(move |_trigger, _window, _cx| {
            Button::new("model").ghost().small().label(label.clone()).dropdown_caret(true).tooltip("Model")
        })
        .footer(|_window, _cx| {
            Button::new("manage-models")
                .ghost()
                .small()
                .icon(IconName::Settings)
                .label("Manage models in Settings")
                .on_click(|_event, window, app| settings::open(window, app))
        })
}

/// Send, or while the agent works a spinner that stops it.
fn send_button(running: bool, can_send: bool, cx: &mut Context<ChatView>) -> impl IntoElement {
    if running {
        return div()
            .id("stop")
            .size(px(SEND_SIZE))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(theme::RADIUS_CONTROL))
            .bg(color(theme::ACCENT))
            .cursor_pointer()
            .hover(|style| style.bg(color(theme::ACCENT_HOVER)))
            .tooltip(|window, cx| {
                gpui_kit::component::tooltip::Tooltip::new("Stop").build(window, cx)
            })
            .on_click(cx.listener(|chat, _event, _window, cx| chat.cancel(cx)))
            .child(
                Spinner::new()
                    .small()
                    .color(Hsla::from(rgb(theme::ON_ACCENT))),
            )
            .into_any_element();
    }
    Button::new("send")
        .primary()
        .small()
        .icon(IconName::ArrowUp)
        .tooltip("Send")
        .disabled(!can_send)
        .on_click(cx.listener(|chat, _event, window, cx| chat.send(window, cx)))
        .into_any_element()
}
