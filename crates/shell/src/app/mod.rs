//! The app window: title bar, then Home or a chat with its rail.

mod titlebar;

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::prelude::*;
use gpui_kit::{Context, Entity, Focusable, Subscription, Window, div, px};
use tokio::runtime::Runtime;
use vivianne_providers::env::Selection;

use crate::agent_setup;
use crate::chat::{ChatHost, ChatView};
use crate::home::{HomeEvent, HomeView, ThreadRow};
use crate::rail::{Rail, RailEvent};
use crate::theme::{self, color};


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Home,
    /// Index into `AppView::threads`.
    Thread(usize),
}

struct Thread {
    chat: Entity<ChatView>,
}

pub struct AppView {
    runtime: Arc<Runtime>,
    workspace: PathBuf,
    threads: Vec<Thread>,
    page: Page,
    home: Entity<HomeView>,
    rail: Entity<Rail>,
    rail_open: bool,
    /// Threads kept on disk for this project, read when Home is shown.
    saved: Vec<vivianne_runtime::store::Summary>,
    _home_events: Subscription,
    _rail_events: Subscription,
}

impl AppView {
    pub fn new(
        runtime: Arc<Runtime>,
        workspace: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let home = cx.new(|cx| HomeView::new(workspace.clone(), window, cx));
        let rail = cx.new(|_| Rail::new(workspace.clone()));
        let home_events =
            cx.subscribe_in(&home, window, |app, _home, event, window, cx| match event {
                HomeEvent::NewThread => app.new_thread(window, cx),
                HomeEvent::OpenThread(index) => app.open_thread(*index, window, cx),
                HomeEvent::OpenSaved(id) => app.open_saved(id, window, cx),
            });
        let rail_events = cx.subscribe(&rail, |app, _rail, event, cx| match event {
            RailEvent::UndoLastRun => app.undo_last_run(cx),
        });
        let mut app = Self {
            runtime,
            workspace,
            threads: Vec::new(),
            page: Page::Home,
            home,
            rail,
            rail_open: true,
            saved: Vec::new(),
            _home_events: home_events,
            _rail_events: rail_events,
        };
        app.reload_saved();
        crate::backend::start_memory(vec![app.workspace.display().to_string()]);
        app.new_thread(window, cx);
        app
    }

    fn new_thread(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (session, selection) = match agent_setup::new_session(&self.runtime, &self.workspace) {
            Ok((session, selection)) => (Ok(session), selection),
            Err(problem) => (Err(problem), Selection { provider: String::new(), model: "not configured".to_owned() }),
        };
        let project = self.workspace.file_name().map_or_else(
            || self.workspace.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let host = ChatHost { runtime: self.runtime.clone(), workspace: self.workspace.clone() };
        let chat = cx.new(|cx| ChatView::new(session, selection, project, host, window, cx));
        // Redraw the tab title and status whenever the chat changes.
        cx.observe(&chat, |_app, _chat, cx| cx.notify()).detach();
        vivianne_telemetry::track("thread.new", serde_json::json!({}));
        self.threads.push(Thread { chat });
        self.open_thread(self.threads.len() - 1, window, cx);
    }

    /// Opens a thread kept on disk, or goes to it if it is already open.
    fn open_saved(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.threads.iter().position(|t| t.chat.read(cx).thread_id() == Some(id)) {
            self.open_thread(index, window, cx);
            return;
        }
        let Some(saved) = vivianne_runtime::store::load(id) else {
            self.reload_saved();
            cx.notify();
            return;
        };
        self.new_thread(window, cx);
        if let Some(thread) = self.threads.last() {
            thread.chat.update(cx, |chat, cx| chat.restore(saved, cx));
        }
    }

    /// The open thread's newest run is undone.
    fn undo_last_run(&mut self, cx: &mut Context<Self>) {
        if let Page::Thread(index) = self.page
            && let Some(thread) = self.threads.get(index)
        {
            thread.chat.update(cx, ChatView::undo_last_run);
        }
    }

    fn reload_saved(&mut self) {
        self.saved = vivianne_runtime::store::list(&self.workspace.display().to_string());
    }

    fn open_thread(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(thread) = self.threads.get(index) else {
            return;
        };
        self.page = Page::Thread(index);
        window.focus(&thread.chat.focus_handle(cx), cx);
        cx.notify();
    }

    fn close_thread(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.threads.len() {
            return;
        }
        self.threads.remove(index);
        self.page = match self.page {
            _ if self.threads.is_empty() => Page::Home,
            Page::Thread(open) if open == index => Page::Thread(index.min(self.threads.len() - 1)),
            Page::Thread(open) if open > index => Page::Thread(open - 1),
            other => other,
        };
        if let Page::Thread(open) = self.page {
            window.focus(&self.threads[open].chat.focus_handle(cx), cx);
        }
        cx.notify();
    }

    /// The Home button: show Home, or go back to the last thread.
    fn toggle_home(&mut self, cx: &mut Context<Self>) {
        self.reload_saved();
        self.page = match (self.page, self.threads.len()) {
            (Page::Home, count) if count > 0 => Page::Thread(count - 1),
            _ => Page::Home,
        };
        cx.notify();
    }

    fn toggle_rail(&mut self, cx: &mut Context<Self>) {
        self.rail_open = !self.rail_open;
        cx.notify();
    }

    fn thread_rows(&self, cx: &Context<Self>) -> Vec<ThreadRow> {
        let mut rows: Vec<ThreadRow> = self
            .threads
            .iter()
            .map(|thread| {
                let chat = thread.chat.read(cx);
                ThreadRow { title: chat.title(), running: chat.is_running(), saved: None }
            })
            .collect();
        let open: Vec<Option<&str>> = self.threads.iter().map(|t| t.chat.read(cx).thread_id()).collect();
        rows.extend(
            self.saved
                .iter()
                .filter(|s| !open.contains(&Some(s.id.as_str())))
                .map(|s| ThreadRow { title: s.title.clone(), running: false, saved: Some(s.id.clone()) }),
        );
        rows
    }

    fn body(&self) -> gpui_kit::AnyElement {
        match self.page {
            Page::Home => div()
                .flex_1()
                .min_h_0()
                .child(self.home.clone())
                .into_any_element(),
            Page::Thread(index) => {
                let mut row = div().flex_1().min_h_0().flex();
                if let Some(thread) = self.threads.get(index) {
                    row = row.child(div().flex_1().min_w_0().child(thread.chat.clone()));
                }
                if self.rail_open {
                    row = row.child(
                        div()
                            .w(px(crate::rail::WIDTH))
                            .flex_none()
                            .h_full()
                            .pt(px(4.0))
                            .pr(px(crate::rail::GAP))
                            .pb(px(crate::rail::GAP))
                            .child(self.rail.clone()),
                    );
                }
                row.into_any_element()
            }
        }
    }
}

impl Render for AppView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.thread_rows(cx);
        self.home.update(cx, |home, cx| home.set_threads(rows, cx));
        if let Page::Thread(index) = self.page
            && let Some(thread) = self.threads.get(index)
        {
            let (context, changes) = {
                let chat = thread.chat.read(cx);
                (chat.context(), chat.changes().cloned())
            };
            self.rail.update(cx, |rail, cx| {
                rail.set_context(context, cx);
                rail.set_changes(changes, cx);
            });
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(color(theme::BG))
            .text_color(color(theme::TEXT))
            .text_size(px(14.0))
            .child(titlebar::render(self, cx))
            .child(self.body())
    }
}
