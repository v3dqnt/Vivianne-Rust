//! The app window: title bar, then Home or a chat with its rail.

mod session_actions;
mod titlebar;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::prelude::*;
use gpui_kit::{Context, Entity, Focusable, Subscription, Window, div, px};
use tokio::runtime::Runtime;
use vivianne_providers::env::Selection;

use crate::agent_setup;
use crate::chat::{ChatEvent, ChatHost, ChatView};
use crate::git::{GitEvent, GitView};
use crate::home::{HomeEvent, HomeView, ThreadRow};
use crate::pair::{PairEvent, PairView};
use crate::rail::{Rail, RailEvent};
use crate::theme::{self, color};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Home,
    /// The project's repository.
    Git,
    /// Working on the project with a partner.
    Pair,
    /// Index into `AppView::threads`.
    Thread(usize),
}

struct Thread {
    chat: Entity<ChatView>,
}

/// The views that belong to one project: its Home, its side panel and its repository.
struct Views {
    home: Entity<HomeView>,
    rail: Entity<Rail>,
    git: Entity<GitView>,
    pair: Entity<PairView>,
    _events: [Subscription; 4],
}

pub struct AppView {
    runtime: Arc<Runtime>,
    /// The project new threads start in.
    workspace: PathBuf,
    threads: Vec<Thread>,
    page: Page,
    views: Views,
    rail_open: bool,
    /// Threads kept on disk for this project, read when Home is shown.
    saved: Vec<vivianne_runtime::store::Summary>,
}

impl AppView {
    pub fn new(
        runtime: Arc<Runtime>,
        workspace: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // The folder the app was started in is a project from now on.
        let _ = vivianne_runtime::projects::add_folder(&workspace);
        let views = Self::views(&workspace, window, cx);
        let mut app = Self {
            runtime,
            workspace,
            threads: Vec::new(),
            page: Page::Home,
            views,
            rail_open: true,
            saved: Vec::new(),
        };
        app.reload_saved();
        crate::backend::start_memory(vec![app.workspace.display().to_string()]);
        app.new_thread(window, cx);
        Self::start_scheduler(window, cx);
        app.offer_resumable(window, cx);
        app
    }

    fn views(workspace: &Path, window: &mut Window, cx: &mut Context<Self>) -> Views {
        let home = cx.new(|cx| HomeView::new(workspace.to_path_buf(), window, cx));
        let rail = cx.new(|_| Rail::new(workspace.to_path_buf()));
        let git = cx.new(|cx| GitView::new(workspace.to_path_buf(), cx));
        let pair = cx.new(|cx| PairView::new(workspace.to_path_buf(), window, cx));
        let pair_events =
            cx.subscribe_in(&pair, window, |app, _pair, event, window, cx| match event {
                PairEvent::Back => app.leave_pair(window, cx),
            });
        let home_events =
            cx.subscribe_in(&home, window, |app, _home, event, window, cx| match event {
                HomeEvent::NewThread => app.new_thread(window, cx),
                HomeEvent::OpenThread(index) => app.open_thread(*index, window, cx),
                HomeEvent::OpenSaved(id) => app.open_saved(id, window, cx),
                HomeEvent::SwitchProject(folder) => app.switch_project(folder.clone(), window, cx),
                HomeEvent::BrowseProject(folder) => app.browse_project(folder.clone(), window, cx),
            });
        let rail_events =
            cx.subscribe_in(&rail, window, |app, _rail, event, window, cx| match event {
                RailEvent::UndoLastRun => app.undo_last_run(cx),
                RailEvent::OpenGit => app.toggle_git(window, cx),
                RailEvent::OpenPair => app.open_pair(cx),
            });
        let git_events =
            cx.subscribe_in(&git, window, |app, _git, event, window, cx| match event {
                GitEvent::Back => app.leave_git(window, cx),
                GitEvent::AskAgent(text) => app.ask_agent(text.clone(), window, cx),
            });
        // The rail shows what the repository needs, so it follows the repository.
        cx.observe(&git, |_app, _git, cx| cx.notify()).detach();
        Views {
            home,
            rail,
            git,
            pair,
            _events: [home_events, rail_events, git_events, pair_events],
        }
    }

    /// Works in another project: its Home, panel and repository replace the old ones, and a thread starts in it.
    /// Threads open in the old project stay open, each in its own folder.
    /// Changes the project without starting a thread in it: the page stays, now showing that project's threads.
    fn browse_project(&mut self, folder: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if folder == self.workspace {
            return;
        }
        self.workspace = folder;
        self.views = Self::views(&self.workspace, window, cx);
        self.reload_saved();
        crate::backend::start_memory(vec![self.workspace.display().to_string()]);
        cx.notify();
    }

    fn switch_project(&mut self, folder: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if folder == self.workspace {
            return;
        }
        // The thread the choice was made in makes way for the new one, if nothing was said in it.
        if let Page::Thread(index) = self.page
            && self
                .threads
                .get(index)
                .is_some_and(|t| t.chat.read(cx).is_empty())
        {
            self.threads.remove(index);
        }
        self.workspace = folder;
        self.views = Self::views(&self.workspace, window, cx);
        self.reload_saved();
        crate::backend::start_memory(vec![self.workspace.display().to_string()]);
        self.new_thread(window, cx);
    }

    fn new_thread(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let index = self.add_thread(window, cx);
        self.open_thread(index, window, cx);
    }

    /// Starts a thread in the project without going to it. Its index.
    fn add_thread(&mut self, window: &mut Window, cx: &mut Context<Self>) -> usize {
        let (session, selection) = match agent_setup::new_session(&self.runtime, &self.workspace) {
            Ok((session, selection)) => (Ok(session), selection),
            Err(problem) => (
                Err(problem),
                Selection {
                    provider: String::new(),
                    model: "not configured".to_owned(),
                },
            ),
        };
        let project = self.workspace.file_name().map_or_else(
            || self.workspace.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let host = ChatHost {
            runtime: self.runtime.clone(),
            workspace: self.workspace.clone(),
        };
        let chat = cx.new(|cx| ChatView::new(session, selection, project, host, window, cx));
        // Redraw the tab title and status whenever the chat changes.
        cx.observe(&chat, |_app, _chat, cx| cx.notify()).detach();
        cx.subscribe_in(&chat, window, |app, _chat, event, window, cx| match event {
            ChatEvent::SwitchProject(folder) => app.switch_project(folder.clone(), window, cx),
        })
        .detach();
        chat.update(cx, ChatView::refresh_branch);
        vivianne_telemetry::track("thread.new", serde_json::json!({}));
        self.threads.push(Thread { chat });
        self.threads.len() - 1
    }

    /// Tasks that were running or waiting when the app last closed, in this project: each is opened in the thread it
    /// was in, with the message that resumes it ready to send. Nothing runs until the person sends it.
    fn offer_resumable(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let workspace = self.workspace.display().to_string();
        for task in vivianne_runtime::ledger::resumable(&workspace, |id| {
            vivianne_runtime::store::load(id).is_some()
        }) {
            let Some(saved) = vivianne_runtime::store::load(&task.key) else {
                continue;
            };
            // Offered once: whether or not it is taken up, it is not a cut-off run on the next launch too.
            vivianne_runtime::ledger::finish(
                &task.key,
                vivianne_runtime::ledger::Outcome::Paused(
                    "interrupted when the app closed".to_owned(),
                ),
            );
            let index = self.add_thread(window, cx);
            self.threads[index].chat.update(cx, |chat, cx| {
                chat.restore(saved, cx);
                chat.offer_resume(window, cx);
            });
        }
    }

    /// An automation fired: it runs in a thread of its own, started in the background so what the person is
    /// looking at stays.
    fn run_automation(
        &mut self,
        fired: vivianne_runtime::automation::Fired,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let automation = fired.automation;
        let prompt = match &fired.file {
            Some(file) => format!(
                "{}\n\nThe file that set this off: {file}",
                automation.prompt
            ),
            None => automation.prompt.clone(),
        };
        let index = self.add_thread(window, cx);
        vivianne_telemetry::track("automation.fired", serde_json::json!({}));
        self.threads[index].chat.update(cx, |chat, cx| {
            chat.run_automation(&automation.name, &prompt, automation.steps, window, cx)
        });
        cx.notify();
    }

    /// Looks every half minute for automations that are due and runs them.
    fn start_scheduler(window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            loop {
                let due = crate::backend::spawn(async {
                    vivianne_runtime::automation::due(vivianne_runtime::automation::now_ms())
                });
                if let Some(due) = due
                    && let Ok(due) = due.await
                    && !due.is_empty()
                    && this
                        .update_in(cx, |app, window, cx| {
                            for fired in due {
                                app.run_automation(fired, window, cx);
                            }
                        })
                        .is_err()
                {
                    return;
                }
                let scheduled = crate::backend::spawn(async {
                    let now = i64::try_from(vivianne_runtime::automation::now_ms()).unwrap_or(i64::MAX);
                    vivianne_runtime::schedule::store::Store::standard().due(now)
                });
                if let Some(scheduled) = scheduled
                    && let Ok(Ok(scheduled)) = scheduled.await
                    && !scheduled.is_empty()
                    && this
                        .update_in(cx, |app, window, cx| {
                            for due in scheduled {
                                app.run_scheduled(due, window, cx);
                            }
                        })
                        .is_err()
                {
                    return;
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(30))
                    .await;
            }
        })
        .detach();
    }

    /// Opens a thread kept on disk, or goes to it if it is already open.
    fn open_saved(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self
            .threads
            .iter()
            .position(|t| t.chat.read(cx).thread_id() == Some(id))
        {
            self.open_thread(index, window, cx);
            return;
        }
        // Reading and parsing a long thread from disk is the slow part of opening it; it runs off the UI thread so
        // the window keeps drawing frames (R2.1) and the thread appears when it is ready.
        let id = id.to_owned();
        let loaded = crate::backend::spawn(async move { vivianne_runtime::store::load(&id) });
        cx.spawn_in(window, async move |this, cx| {
            let saved = match loaded {
                Some(rx) => rx.await.ok().flatten(),
                None => None,
            };
            this.update_in(cx, |app, window, cx| {
                let Some(saved) = saved else {
                    app.reload_saved();
                    cx.notify();
                    return;
                };
                app.new_thread(window, cx);
                if let Some(thread) = app.threads.last() {
                    thread.chat.update(cx, |chat, cx| chat.restore(saved, cx));
                }
            })
            .ok();
        })
        .detach();
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

    /// The Git tab: show the repository, or go back to the last thread.
    fn toggle_git(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.page == Page::Git {
            self.leave_git(window, cx);
        } else {
            self.page = Page::Git;
            cx.notify();
        }
    }

    /// The Pair page.
    fn open_pair(&mut self, cx: &mut Context<Self>) {
        self.page = Page::Pair;
        self.views.pair.update(cx, PairView::reload);
        cx.notify();
    }

    fn leave_pair(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_git(window, cx);
    }

    fn leave_git(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.threads.len() {
            0 => {
                self.page = Page::Home;
                cx.notify();
            }
            count => self.open_thread(count - 1, window, cx),
        }
    }

    /// Hands the open thread a job and shows it; a thread is started if there is none.
    fn ask_agent(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.threads.is_empty() {
            self.new_thread(window, cx);
        }
        let index = match self.page {
            Page::Thread(index) if index < self.threads.len() => index,
            _ => self.threads.len() - 1,
        };
        self.open_thread(index, window, cx);
        self.threads[index]
            .chat
            .update(cx, |chat, cx| chat.ask(&text, window, cx));
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
                ThreadRow {
                    title: chat.title(),
                    running: chat.is_running(),
                    saved: None,
                }
            })
            .collect();
        let open: Vec<Option<&str>> = self
            .threads
            .iter()
            .map(|t| t.chat.read(cx).thread_id())
            .collect();
        rows.extend(
            self.saved
                .iter()
                .filter(|s| !open.contains(&Some(s.id.as_str())))
                .map(|s| ThreadRow {
                    title: s.title.clone(),
                    running: false,
                    saved: Some(s.id.clone()),
                }),
        );
        rows
    }

    fn body(&self) -> gpui_kit::AnyElement {
        match self.page {
            Page::Home => div()
                .flex_1()
                .min_h_0()
                .bg(color(theme::BG))
                .child(self.views.home.clone())
                .into_any_element(),
            Page::Pair => div()
                .flex_1()
                .min_h_0()
                .bg(color(theme::BG))
                .child(self.views.pair.clone())
                .into_any_element(),
            Page::Git => div()
                .flex_1()
                .min_h_0()
                .bg(color(theme::BG))
                .child(self.views.git.clone())
                .into_any_element(),
            Page::Thread(index) => {
                let mut row = div().flex_1().min_h_0().flex();
                if let Some(thread) = self.threads.get(index) {
                    // Where the page meets the rail its corner is rounded the other way round: the glass of the
                    // header runs down into the rail's edge, so the two read as one surface.
                    let corner = if self.rail_open {
                        px(theme::RADIUS_PANEL)
                    } else {
                        px(0.0)
                    };
                    row = row.child(
                        div().flex_1().min_w_0().bg(theme::glass()).child(
                            div()
                                .size_full()
                                .bg(color(theme::BG))
                                .rounded_tr(corner)
                                .overflow_hidden()
                                .child(thread.chat.clone()),
                        ),
                    );
                }
                if self.rail_open {
                    row = row.child(
                        div()
                            .w(px(crate::rail::WIDTH))
                            .flex_none()
                            .h_full()
                            .child(self.views.rail.clone()),
                    );
                }
                row.into_any_element()
            }
        }
    }
}

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::perf::record_frame();
        if crate::perf::continuous() {
            window.request_animation_frame();
        }
        let rows = self.thread_rows(cx);
        self.views
            .home
            .update(cx, |home, cx| home.set_threads(rows, cx));
        if let Page::Thread(index) = self.page
            && let Some(thread) = self.threads.get(index)
        {
            let (context, changes, summary) = {
                let chat = thread.chat.read(cx);
                (
                    chat.context(),
                    chat.changes().cloned(),
                    chat.change_summary().cloned(),
                )
            };
            let badge = self.views.git.read(cx).badge();
            self.views.rail.update(cx, |rail, cx| {
                rail.set_badge(badge, cx);
                rail.set_context(context, cx);
                rail.set_changes(changes, summary, cx);
            });
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .text_color(color(theme::TEXT))
            .text_size(px(14.0))
            .child(titlebar::render(self, cx))
            .child(self.body())
    }
}
