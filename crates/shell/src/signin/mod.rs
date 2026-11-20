//! A window that signs a coding agent in: the agent's own sign-in command in a terminal, and a check of its
//! status that repeats until the agent is signed in.

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::TitleBar;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::prelude::*;
use gpui_kit::{App, Bounds, Context, Entity, Window, WindowOptions, div, px, size};
use vivianne_providers::acp::{self, AuthState, Depth};

use crate::backend;
use crate::rail::terminal::TerminalView;
use crate::theme::{self, color};

/// Where the terminal's size comes from: the window is made to fit it.
const COLUMNS: u16 = 104;
const LINES: u16 = 26;
const WINDOW_WIDTH: f32 = 780.0;
const WINDOW_HEIGHT: f32 = 520.0;
/// How long to wait before the first check, and between checks: signing in takes the user a while, and each
/// check starts the agent.
const FIRST_CHECK_AFTER: Duration = Duration::from_secs(6);
const CHECK_EVERY: Duration = Duration::from_secs(6);
/// How long "Signed in" stays up before the window closes.
const CLOSE_AFTER: Duration = Duration::from_millis(1400);

/// What the window says about the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Status {
    Waiting,
    Checking,
    SignedIn,
    Problem(String),
}

/// What to do once the agent is signed in.
pub type OnSignedIn = Rc<dyn Fn(&mut App)>;

/// Opens the sign-in window for the agent `id` (shown as `name`), running `command` in a terminal. `done` runs
/// when the agent is found signed in.
pub fn open(id: &str, name: &str, command: &str, done: OnSignedIn, cx: &mut App) {
    let (id, name, command) = (id.to_owned(), name.to_owned(), command.to_owned());
    let bounds = Bounds::centered(None, size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), cx);
    let options = WindowOptions { window_bounds: Some(gpui_kit::WindowBounds::Windowed(bounds)), ..TitleBar::window_options() };
    let opened = gpui_kit::open_window(options, cx, move |window, cx| cx.new(|cx| LoginView::new(id, name, &command, done, window, cx)));
    if let Err(error) = opened {
        eprintln!("vivianne: cannot open the sign-in window: {error}");
    }
}

struct LoginView {
    id: String,
    name: String,
    terminal: Entity<TerminalView>,
    status: Status,
    done: OnSignedIn,
}

impl LoginView {
    fn new(id: String, name: String, command: &str, done: OnSignedIn, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let command = command.to_owned();
        let terminal = cx.new(|cx| TerminalView::running(&root, &command, (COLUMNS, LINES), cx));
        let focus = terminal.read(cx).focus_handle().clone();
        window.focus(&focus, cx);
        // Check now and then while the window is open. The loop ends when the window does.
        cx.spawn_in(window, async move |view, cx| {
            cx.background_executor().timer(FIRST_CHECK_AFTER).await;
            loop {
                let again = view.update_in(cx, |view, window, cx| {
                    view.check_once(window, cx);
                    view.status != Status::SignedIn
                });
                if !again.unwrap_or(false) {
                    break;
                }
                cx.background_executor().timer(CHECK_EVERY).await;
            }
        })
        .detach();
        Self { id, name, terminal, status: Status::Waiting, done }
    }

    /// Asks the agent whether it is signed in now, and shows the answer.
    fn check_once(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.status, Status::Checking | Status::SignedIn) {
            return;
        }
        let id = self.id.clone();
        let Some(answer) = backend::spawn(async move { acp::check_auth(&id, Depth::Quick).await }) else {
            self.status = Status::Problem("The app's services didn't start.".to_owned());
            return;
        };
        self.status = Status::Checking;
        cx.notify();
        cx.spawn_in(window, async move |view, cx| {
            let Ok(result) = answer.await else { return };
            let _ = view.update_in(cx, |view, window, cx| view.checked(result, window, cx));
        })
        .detach();
    }

    fn checked(&mut self, result: Result<AuthState, String>, window: &mut Window, cx: &mut Context<Self>) {
        match result {
            Ok(AuthState::SignedIn) => {
                self.status = Status::SignedIn;
                (self.done)(cx);
                cx.spawn_in(window, async move |_view, cx| {
                    cx.background_executor().timer(CLOSE_AFTER).await;
                    let _ = cx.update(|window, _cx| window.remove_window());
                })
                .detach();
            }
            Ok(AuthState::NeedsSignIn(_)) => self.status = Status::Waiting,
            Ok(AuthState::Failed(why)) | Err(why) => self.status = Status::Problem(why),
        }
        cx.notify();
    }
}

impl Render for LoginView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (text, tint) = match &self.status {
            Status::Waiting => ("Waiting for you to sign in. This window checks by itself.".to_owned(), theme::MUTED),
            Status::Checking => ("Checking whether it is signed in…".to_owned(), theme::MUTED),
            Status::SignedIn => (format!("{} is signed in. Closing…", self.name), theme::OK),
            Status::Problem(why) => (why.clone(), theme::DANGER),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .p(px(14.0))
            .bg(color(theme::BG))
            .text_color(color(theme::TEXT))
            .text_size(px(13.0))
            .child(div().child(format!("Sign in to {}. Follow what it asks in the terminal below.", self.name)))
            .child(div().flex_1().min_h_0().child(self.terminal.clone()))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(10.0))
                    .child(div().text_color(color(tint)).child(text))
                    .child(
                        div()
                            .flex()
                            .gap(px(8.0))
                            .child(Button::new("signin-check").label("Check now").on_click(cx.listener(|view, _event, window, cx| view.check_once(window, cx))))
                            .child(Button::new("signin-close").label("Close").ghost().on_click(|_event, window, _cx| window.remove_window())),
                    ),
            )
    }
}
