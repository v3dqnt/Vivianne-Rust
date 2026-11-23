//! Browser: lets the agent open and use web pages in a browser of its own, through `agent-browser`. Off until switched
//! on here; the program is installed only from this page, from the project's own GitHub release, after its checksum is
//! checked. It uses Chrome or Edge if they are installed and never the user's own profile.

use std::sync::Mutex;

use gpui_kit::base::{Disableable as _, h_flex};
use gpui_kit::component::IconName;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::Button;
use gpui_kit::component::setting::{SettingGroup, SettingItem, SettingPage};
use gpui_kit::prelude::*;
use gpui_kit::{Styled, div};
use vivianne_tools::browser;
use vivianne_tools::computer::Release;

use crate::settings::values::Shared;
use crate::settings::widgets::{keyed_input, muted, strong};

const DOMAINS: &str = "browser-domains";

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

pub fn page(values: &Shared) -> SettingPage {
    let values = values.clone();
    SettingPage::new("Browser")
        .icon(IconName::Globe)
        .description("Let the agent open and use web pages. It works in a browser of its own, separate from yours: your logins, cookies and history are never shared with it.")
        .group(
            SettingGroup::new()
                .title("Status")
                .item(SettingItem::render(|_o, _w, _cx| {
                    status().into_any_element()
                })),
        )
        .group(
            SettingGroup::new()
                .title("Allowed sites")
                .description("Limit the browser to these sites, separated by commas (example.com, *.example.org). Empty means any site.")
                .item(SettingItem::render(move |_o, window, cx| {
                    let values = values.clone();
                    let input = keyed_input(
                        DOMAINS,
                        &values,
                        |v| {
                            v.key_drafts
                                .get(DOMAINS)
                                .cloned()
                                .unwrap_or_else(|| browser::setting().allowed_domains)
                        },
                        |v, x| {
                            v.key_drafts.insert(DOMAINS.to_owned(), x);
                        },
                        "example.com, *.example.org",
                        window,
                        cx,
                    );
                    let saved = values.clone();
                    h_flex()
                        .gap_2()
                        .child(input)
                        .child(
                            Button::new("browser-domains-save")
                                .outline()
                                .small()
                                .label("Save")
                                .on_click(move |_e, window, cx| {
                                    let text = saved
                                        .borrow()
                                        .key_drafts
                                        .get(DOMAINS)
                                        .cloned()
                                        .unwrap_or_default();
                                    browser::update(|s| s.allowed_domains = text);
                                    reconnect(cx);
                                    window.refresh();
                                }),
                        )
                        .into_any_element()
                })),
        )
        .group(
            SettingGroup::new()
                .title("What it can do")
                .item(SettingItem::render(|_o, _w, _cx| {
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(muted("It can open pages, read them, click, type, scroll and go back."))
                        .child(muted("Opening a page asks first, and so does everything else in Ask mode. Reading the page that is already open does not."))
                        .child(muted("What a page says is treated as text from strangers, never as instructions. Nothing a page showed is saved in threads or memory."))
                        .child(muted("It will not type a key, token or password, run scripts on a page, upload files, or use your browser profile."))
                        .into_any_element()
                })),
        )
}

/// Reconnects the server so a changed setting takes effect.
fn reconnect(cx: &mut gpui_kit::App) {
    crate::backend::run(cx, browser::apply(), |result, cx| {
        if let Err(e) = result {
            set_message(e);
        }
        cx.refresh_windows();
    });
    cx.refresh_windows();
}

fn status() -> impl IntoElement {
    let s = browser::setting();
    let binary = browser::installed_binary();
    let (release, message, busy) = {
        let st = state();
        (st.release.clone(), st.message.clone(), st.busy)
    };
    let mut column = div().flex().flex_col().gap_2();
    column = column.child(strong(if s.enabled { "On" } else { "Off" }));
    column = column.child(muted(binary.as_ref().map_or_else(
        || "agent-browser is not installed.".to_owned(),
        |p| format!("agent-browser: {}", p.display()),
    )));
    column = column.child(muted(browser::system_browser().map_or_else(
        || "No Chrome or Edge found: it would need to download its own browser.".to_owned(),
        |p| format!("Browser: {}", p.display()),
    )));
    column = column.child(muted(if s.auto {
        "Auto: it acts on pages without asking. Opening a page still asks unless you limited the sites."
    } else {
        "It asks before opening a page or acting on one (in Ask mode)."
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
    let on = s.enabled;
    buttons = buttons.child(
        Button::new("browser-toggle")
            .outline()
            .small()
            .label(if on { "Turn off" } else { "Turn on" })
            .on_click(move |_e, window, cx| {
                browser::update(|s| s.enabled = !on);
                set_message(String::new());
                reconnect(cx);
                window.refresh();
            }),
    );
    let auto = s.auto;
    buttons = buttons.child(
        Button::new("browser-auto")
            .outline()
            .small()
            .label(if auto {
                "Ask before acting"
            } else {
                "Auto mode"
            })
            .on_click(move |_e, window, cx| {
                browser::update(|s| s.auto = !auto);
                window.refresh();
                cx.refresh_windows();
            }),
    );
    let headed = s.headed;
    buttons = buttons.child(
        Button::new("browser-headed")
            .outline()
            .small()
            .label(if headed {
                "Hide the window"
            } else {
                "Show the window"
            })
            .on_click(move |_e, window, cx| {
                browser::update(|s| s.headed = !headed);
                reconnect(cx);
                window.refresh();
            }),
    );
    buttons = buttons.child(
        Button::new("browser-find")
            .outline()
            .small()
            .disabled(busy)
            .label("Find latest release")
            .on_click(|_e, window, cx| {
                begin("Looking for the latest release…");
                crate::backend::run(cx, browser::find_release(), |result, cx| {
                    let mut st = state();
                    st.busy = false;
                    match result {
                        Ok(r) => {
                            st.message = format!(
                                "Found {}. Install puts it in ~/.viv/bin after checking its checksum.",
                                r.version
                            );
                            st.release = Some(r);
                        }
                        Err(e) => st.message = e,
                    }
                    drop(st);
                    cx.refresh_windows();
                });
                window.refresh();
            }),
    );
    if let Some(r) = release {
        buttons = buttons.child(
            Button::new("browser-install")
                .small()
                .disabled(busy)
                .label(format!("Install {}", r.version))
                .on_click(move |_e, window, cx| {
                    begin("Downloading and checking…");
                    let r = r.clone();
                    crate::backend::run(
                        cx,
                        async move { browser::download(&r).await },
                        |result, cx| {
                            let mut st = state();
                            st.busy = false;
                            st.message = match result {
                                Ok(path) => format!("Installed at {}.", path.display()),
                                Err(e) => e,
                            };
                            drop(st);
                            if browser::enabled() {
                                reconnect(cx);
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
            Button::new("browser-check")
                .outline()
                .small()
                .disabled(busy)
                .label("Check")
                .on_click(move |_e, window, cx| {
                    begin("Checking…");
                    let path = path.clone();
                    crate::backend::run(
                        cx,
                        async move { browser::status(&path).await },
                        |result, cx| {
                            let mut st = state();
                            st.busy = false;
                            st.message = result.unwrap_or_else(|e| e);
                            drop(st);
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
