//! Computer use: lets the agent see and operate desktop apps through `agent-desktop`. Off until switched on here; the
//! program is installed only from this page, from the project's own GitHub release, after its checksum is checked.

use std::sync::Mutex;

use gpui_kit::base::Disableable as _;
use gpui_kit::component::IconName;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::Button;
use gpui_kit::component::setting::{SettingGroup, SettingItem, SettingPage};
use gpui_kit::prelude::*;
use gpui_kit::{Styled, div};
use vivianne_tools::computer::{self, Release};

use crate::settings::values::Shared;
use crate::settings::widgets::{muted, strong};

/// What the page last learned. It is shared because the buttons finish on another thread.
#[derive(Default)]
struct State {
    release: Option<Release>,
    message: String,
    busy: bool,
}

static STATE: Mutex<State> = Mutex::new(State {
    release: None,
    message: String::new(),
    busy: false,
});

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub fn page(_values: &Shared) -> SettingPage {
    SettingPage::new("Computer use")
        .icon(IconName::Bot)
        .description("Let the agent see and operate desktop apps. It reads each app's accessibility tree, not pixels, and works in the background without moving your mouse.")
        .group(SettingGroup::new().title("Status").item(SettingItem::render(|_o, _w, _cx| status().into_any_element())))
        .group(SettingGroup::new().title("What it can do").item(SettingItem::render(|_o, _w, _cx| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(muted("It can read what an app shows, click, type, press keys, open and close programs."))
                .child(muted("Anything that changes something asks first in Ask mode, and nothing but looking runs in Plan mode."))
                .child(muted("It will not type a key, token or password. What it sees is never saved in threads or memory."))
                .into_any_element()
        })))
}

fn status() -> impl IntoElement {
    let on = computer::enabled();
    let binary = computer::installed_binary();
    let (release, message, busy) = {
        let s = state();
        (s.release.clone(), s.message.clone(), s.busy)
    };
    let mut column = div().flex().flex_col().gap_2();
    column = column.child(strong(if on { "On" } else { "Off" }));
    column = column.child(muted(binary.as_ref().map_or_else(
        || "agent-desktop is not installed.".to_owned(),
        |p| format!("agent-desktop: {}", p.display()),
    )));
    let auto = computer::auto();
    column = column.child(muted(if auto {
        "Auto: it acts on apps without asking (Plan mode still refuses)."
    } else {
        "It asks before acting on an app (in Ask mode)."
    }));
    if let Some(r) = &release {
        column = column.child(muted(format!(
            "Latest: {} ({:.1} MB), SHA-256 {}…",
            r.version,
            r.size as f64 / 1_048_576.0,
            &r.sha256[..12.min(r.sha256.len())]
        )));
    }
    if !message.is_empty() {
        column = column.child(muted(message));
    }
    let mut buttons = div().flex().gap_1().flex_wrap();
    buttons = buttons.child(
        Button::new("computer-toggle")
            .outline()
            .small()
            .label(if on { "Turn off" } else { "Turn on" })
            .on_click(move |_e, window, cx| {
                computer::set_enabled(!on);
                set_message(String::new());
                crate::backend::run(cx, computer::apply(), |result, cx| {
                    if let Err(e) = result {
                        set_message(e);
                    }
                    cx.refresh_windows();
                });
                window.refresh();
                cx.refresh_windows();
            }),
    );
    buttons = buttons.child(
        Button::new("computer-auto")
            .outline()
            .small()
            .label(if auto {
                "Ask before acting"
            } else {
                "Auto mode"
            })
            .on_click(move |_e, window, cx| {
                computer::set_auto(!auto);
                window.refresh();
                cx.refresh_windows();
            }),
    );
    buttons = buttons.child(
        Button::new("computer-find")
            .outline()
            .small()
            .disabled(busy)
            .label("Find latest release")
            .on_click(|_e, window, cx| {
                begin("Looking for the latest release…");
                crate::backend::run(cx, computer::find_release(), |result, cx| {
                    let mut s = state();
                    s.busy = false;
                    match result {
                        Ok(r) => {
                            s.message = format!("Found {}. Install puts it in ~/.viv/bin after checking its checksum.", r.version);
                            s.release = Some(r);
                        }
                        Err(e) => s.message = e,
                    }
                    drop(s);
                    cx.refresh_windows();
                });
                window.refresh();
            }),
    );
    if let Some(r) = release {
        buttons = buttons.child(
            Button::new("computer-install")
                .small()
                .disabled(busy)
                .label(format!("Install {}", r.version))
                .on_click(move |_e, window, cx| {
                    begin("Downloading and checking…");
                    let r = r.clone();
                    crate::backend::run(
                        cx,
                        async move { computer::download(&r).await },
                        |result, cx| {
                            let mut s = state();
                            s.busy = false;
                            s.message = match result {
                                Ok(path) => format!("Installed at {}.", path.display()),
                                Err(e) => e,
                            };
                            drop(s);
                            if computer::enabled() {
                                crate::backend::run(cx, computer::apply(), |_, cx| {
                                    cx.refresh_windows()
                                });
                            }
                            cx.refresh_windows();
                        },
                    );
                    window.refresh();
                }),
        );
    }
    if let Some(path) = binary {
        buttons = buttons.child(
            Button::new("computer-check")
                .outline()
                .small()
                .disabled(busy)
                .label("Check")
                .on_click(move |_e, window, cx| {
                    begin("Checking…");
                    let path = path.clone();
                    crate::backend::run(
                        cx,
                        async move { computer::status(&path).await },
                        |result, cx| {
                            let mut s = state();
                            s.busy = false;
                            s.message = match result {
                                Ok(st) => format!(
                                    "Version {} on {}, accessibility: {}.",
                                    st.version, st.platform, st.accessibility
                                ),
                                Err(e) => e,
                            };
                            drop(s);
                            cx.refresh_windows();
                        },
                    );
                    window.refresh();
                }),
        );
    }
    column.child(buttons)
}

fn begin(message: &str) {
    let mut s = state();
    s.busy = true;
    s.message = message.to_owned();
}

fn set_message(message: String) {
    state().message = message;
}
