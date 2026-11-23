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

use super::attachments::size_label;
use super::{Mode, Thinking, thinking_label};
use crate::chat::{ChatEvent, ChatView, shorten};
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
        .id("composer")
        .w_full()
        .max_w(px(theme::COLUMN_MAX))
        .flex()
        .flex_col()
        .gap(px(4.0))
        .px(px(14.0))
        .pt(px(12.0))
        .pb(px(8.0))
        .rounded(px(theme::RADIUS_COMPOSER))
        .border_1()
        // Focus lifts the edge a step rather than painting it blue: the caret already says where typing goes.
        .border_color(color(if focused {
            theme::FOCUS_EDGE
        } else {
            theme::BORDER
        }))
        .bg(color(theme::SURFACE))
        .shadow_md()
        .drag_over::<ExternalPaths>(|style, _, _, _| style.border_color(color(theme::ACCENT)))
        .on_drop(cx.listener(|chat, paths: &ExternalPaths, _window, cx| {
            chat.attach_paths(paths.paths(), cx);
        }))
        .when(!composer.attachments.is_empty(), |card| {
            card.child(chips(chat, cx))
        })
        .child(Textarea::new(&composer.input).appearance(false))
        .child(toolbar(chat, can_send, cx))
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
                        .xsmall()
                        .icon(IconName::Plus)
                        .tooltip("Attach files")
                        .on_click(
                            cx.listener(|_chat, _event, _window, cx| ChatView::pick_files(cx)),
                        ),
                )
                .child(mode_menu(chat, cx))
                .child(project_menu(chat, cx)),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(2.0))
                .child(thinking_menu(chat, cx))
                .child(model_menu(chat))
                .children(agent_model_menu(chat, cx))
                .child(
                    Button::new("voice")
                        .ghost()
                        .xsmall()
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
        .xsmall()
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

/// The project the thread works in, with its branch: pick another project, or add a folder.
fn project_menu(chat: &ChatView, cx: &mut Context<ChatView>) -> impl IntoElement {
    let me = cx.entity().downgrade();
    let current = chat.workspace().to_path_buf();
    let (label, branch) = (chat.project.clone(), chat.branch.clone());
    div()
        .flex()
        .items_center()
        .gap(px(4.0))
        .child(
            Button::new("project")
                .ghost()
                .xsmall()
                .icon(IconName::Folder)
                .label(label)
                .dropdown_caret(true)
                .tooltip(current.display().to_string())
                .dropdown_menu_with_anchor(Anchor::BottomLeft, move |menu, _window, _cx| {
                    let menu = vivianne_runtime::projects::list()
                        .into_iter()
                        .filter_map(|p| p.path.map(|path| (p.name, path)))
                        .fold(menu, |menu, (name, path)| {
                            let (me, folder) = (me.clone(), std::path::PathBuf::from(&path));
                            menu.item(
                                PopupMenuItem::new(name)
                                    .checked(folder == current)
                                    .on_click(move |_event, _window, app| {
                                        let folder = folder.clone();
                                        me.update(app, |_chat, cx| {
                                            cx.emit(ChatEvent::SwitchProject(folder))
                                        })
                                        .ok();
                                    }),
                            )
                        });
                    let me = me.clone();
                    menu.separator()
                        .item(PopupMenuItem::new("Add a folder…").on_click(
                            move |_event, _window, app| {
                                me.update(app, |_chat, cx| ChatView::add_project_folder(cx))
                                    .ok();
                            },
                        ))
                }),
        )
        .children(branch.map(|branch| {
            div()
                .text_size(px(theme::TEXT_XS))
                .font_family(theme::MONO)
                .text_color(color(theme::FAINT))
                .child(branch)
        }))
}

fn thinking_menu(chat: &ChatView, cx: &mut Context<ChatView>) -> impl IntoElement {
    let me = cx.entity().downgrade();
    let current = chat.composer.thinking;
    Button::new("thinking")
        .ghost()
        .xsmall()
        .label(thinking_label(current))
        .tooltip("Thinking: how much the model thinks before answering")
        .dropdown_menu_with_anchor(Anchor::BottomRight, move |menu, _window, _cx| {
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

/// For a coding agent: which of its own models it works with (Sonnet or Opus for Claude Code, and so on). The
/// list is what the agent offered when its session opened, or the last time it ran, so it can be empty until the
/// agent has been used once.
fn agent_model_menu(chat: &ChatView, cx: &mut Context<ChatView>) -> Option<impl IntoElement> {
    use vivianne_providers::acp;
    if chat.selection.provider != crate::chat::models::AGENT_PROVIDER {
        return None;
    }
    let harness = chat.selection.model.clone();
    let thread = chat.thread_id().unwrap_or_default().to_owned();
    let live = crate::backend::get().and_then(|b| b.acp.session_info(&thread, &harness));
    let list = live
        .as_ref()
        .map(acp::models_of)
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| acp::known_models(&harness));
    let current = live
        .as_ref()
        .and_then(acp::current_model_id)
        .or_else(|| acp::preferred_model(&harness));
    let label = current
        .as_ref()
        .and_then(|c| list.iter().find(|m| &m.id == c).map(|m| m.name.clone()))
        .or_else(|| current.clone())
        .unwrap_or_else(|| "Default model".to_owned());
    let me = cx.entity().downgrade();
    Some(
        Button::new("agent-model")
            .ghost()
            .xsmall()
            .label(shorten(&label, MODEL_LABEL_CHARS))
            .dropdown_caret(true)
            .tooltip("The agent's own model")
            .dropdown_menu_with_anchor(Anchor::BottomRight, move |menu, _window, _cx| {
                if list.is_empty() {
                    return menu.item(PopupMenuItem::new(
                        "Send a message first: the agent lists its models when it starts",
                    ));
                }
                list.iter().fold(menu, |menu, model| {
                    let (id, thread, harness, me) =
                        (model.id.clone(), thread.clone(), harness.clone(), me.clone());
                    menu.item(
                        PopupMenuItem::new(model.name.clone())
                            .checked(current.as_deref() == Some(model.id.as_str()))
                            .on_click(move |_event, _window, app| {
                                let (id, thread, harness) =
                                    (id.clone(), thread.clone(), harness.clone());
                                if let Some(b) = crate::backend::get() {
                                    let acp = b.acp.clone();
                                    crate::backend::run(
                                        app,
                                        async move { acp.set_model(&thread, &harness, &id).await },
                                        |result, _| {
                                            if let Err(e) = result {
                                                vivianne_core::debuglog::log("app", e);
                                            }
                                        },
                                    );
                                }
                                me.update(app, |_, cx| cx.notify()).ok();
                            }),
                    )
                })
            }),
    )
}

fn model_menu(chat: &ChatView) -> impl IntoElement {
    let label = shorten(&chat.selection.model, MODEL_LABEL_CHARS);
    Combobox::new(&chat.picker)
        .placeholder(label.clone())
        .search_placeholder("Search models")
        .menu_width(px(MODEL_MENU_WIDTH))
        .menu_max_h(px(MODEL_MENU_HEIGHT))
        .appearance(false)
        .empty(|_window, _cx| {
            div()
                .p_3()
                .text_sm()
                .child("No models found. Save a provider key in Settings.")
        })
        .render_trigger(move |_trigger, _window, _cx| {
            Button::new("model")
                .ghost()
                .xsmall()
                .label(label.clone())
                .tooltip("Model")
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
    // While a reply is being written the button stops it: a filled square, the usual sign for stop.
    if running {
        return Button::new("stop")
            .primary()
            .small()
            .icon(IconName::Square)
            .tooltip("Stop")
            .on_click(cx.listener(|chat, _event, _window, cx| chat.cancel(cx)))
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
