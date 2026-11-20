//! The Home page: Projects with their threads, and 24/7 agents.

use std::path::PathBuf;

use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{Icon, IconName};
use gpui_kit::prelude::*;
use gpui_kit::{Context, Entity, EventEmitter, Window, div, px};

use crate::theme::{self, color};

/// What Home asks the app to do.
#[derive(Debug, Clone)]
pub enum HomeEvent {
    NewThread,
    OpenThread(usize),
    /// A thread kept on disk that is not open: its id.
    OpenSaved(String),
}

/// One open thread as Home lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadRow {
    pub title: String,
    pub running: bool,
    /// The id of a thread kept on disk that is not open. `None` for an open thread.
    pub saved: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HomeTab {
    Projects,
    Agents,
}

const COLUMN_MAX: f32 = 1040.0;
const PROJECT_LIST_WIDTH: f32 = 240.0;
const GENERAL: &str = "General";

pub struct HomeView {
    tab: HomeTab,
    /// 0 is General, 1 is the open project folder.
    selected_project: usize,
    project_name: String,
    project_path: PathBuf,
    search: Entity<InputState>,
    threads: Vec<ThreadRow>,
}

impl EventEmitter<HomeEvent> for HomeView {}

impl HomeView {
    pub fn new(project_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let project_name = project_path.file_name().map_or_else(
            || project_path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search threads"));
        Self {
            tab: HomeTab::Projects,
            selected_project: 1,
            project_name,
            project_path,
            search,
            threads: Vec::new(),
        }
    }

    /// Keep the list in step with the open threads; only repaints on change.
    pub fn set_threads(&mut self, threads: Vec<ThreadRow>, cx: &mut Context<Self>) {
        if self.threads != threads {
            self.threads = threads;
            cx.notify();
        }
    }

    fn tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = usize::from(self.tab == HomeTab::Agents);
        TabBar::new("home-tabs")
            .segmented()
            .selected_index(selected)
            .on_click(cx.listener(|home, index: &usize, _window, cx| {
                home.tab = if *index == 0 {
                    HomeTab::Projects
                } else {
                    HomeTab::Agents
                };
                cx.notify();
            }))
            .child(Tab::new().label("Projects"))
            .child(Tab::new().label("24/7 agents"))
    }

    fn project_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let names = [GENERAL.to_owned(), self.project_name.clone()];
        div()
            .w(px(PROJECT_LIST_WIDTH))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pb(px(6.0))
                    .child(div().text_color(color(theme::MUTED)).child("Projects"))
                    .child(
                        Button::new("new-project")
                            .ghost()
                            .icon(IconName::Plus)
                            .disabled(true),
                    ),
            )
            .children(names.into_iter().enumerate().map(|(index, name)| {
                let selected = index == self.selected_project;
                div()
                    .id(("project", index))
                    .px(px(10.0))
                    .py(px(7.0))
                    .rounded(px(theme::RADIUS_CONTROL))
                    .cursor_pointer()
                    .text_color(color(if selected { theme::TEXT } else { theme::MUTED }))
                    .when(selected, |row| row.bg(color(theme::RAISED)))
                    .hover(|style| style.bg(color(theme::SURFACE)).text_color(color(theme::TEXT)))
                    .on_click(cx.listener(move |home, _event, _window, cx| {
                        home.selected_project = index;
                        cx.notify();
                    }))
                    .child(name)
            }))
    }

    fn project_detail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (heading, subtitle) = if self.selected_project == 0 {
            (
                GENERAL.to_owned(),
                "Questions not tied to a project".to_owned(),
            )
        } else {
            (
                self.project_name.clone(),
                self.project_path.display().to_string(),
            )
        };
        let query = self.search.read(cx).value().to_lowercase();
        let rows: Vec<(usize, ThreadRow)> = self
            .threads
            .iter()
            .cloned()
            .enumerate()
            .filter(|(_, row)| query.is_empty() || row.title.to_lowercase().contains(&query))
            .collect();
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(14.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(div().text_size(px(theme::TEXT_TITLE - 2.0)).font_weight(gpui_kit::FontWeight::SEMIBOLD).child(heading))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(color(theme::MUTED))
                                    .child(subtitle),
                            ),
                    )
                    .child(
                        Button::new("home-new-thread")
                            .primary()
                            .label("New thread")
                            .on_click(cx.listener(|_home, _event, _window, cx| {
                                cx.emit(HomeEvent::NewThread);
                            })),
                    ),
            )
            .child(Input::new(&self.search))
            .child(Self::thread_list(rows, cx))
    }

    fn thread_list(rows: Vec<(usize, ThreadRow)>, cx: &mut Context<Self>) -> impl IntoElement {
        if rows.is_empty() {
            return div()
                .py(px(32.0))
                .flex()
                .justify_center()
                .text_color(color(theme::MUTED))
                .child("No threads yet")
                .into_any_element();
        }
        div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .children(rows.into_iter().map(|(index, row)| {
                let status = match (&row.saved, row.running) {
                    (Some(_), _) => "Saved",
                    (None, true) => "Working",
                    (None, false) => "Idle",
                };
                let saved = row.saved.clone();
                div()
                    .id(("thread-row", index))
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(px(40.0))
                    .px(px(12.0))
                    .rounded(px(theme::RADIUS_CONTROL))
                    .cursor_pointer()
                    .hover(|style| style.bg(color(theme::SURFACE)))
                    .on_click(cx.listener(move |_home, _event, _window, cx| {
                        cx.emit(match &saved {
                            Some(id) => HomeEvent::OpenSaved(id.clone()),
                            None => HomeEvent::OpenThread(index),
                        });
                    }))
                    .child(div().child(row.title))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(color(theme::MUTED))
                            .child(status),
                    )
            }))
            .into_any_element()
    }

    fn agents() -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(px(14.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_color(color(theme::MUTED))
                            .child("Agents that keep working on a goal while you are away."),
                    )
                    .child(
                        Button::new("new-agent")
                            .primary()
                            .label("New agent")
                            .disabled(true),
                    ),
            )
            .child(
                div()
                    .py(px(48.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(8.0))
                    .rounded(px(theme::RADIUS_SURFACE))
                    .border_1()
                    .border_color(color(theme::BORDER))
                    .text_color(color(theme::MUTED))
                    .child(Icon::new(IconName::Bot).size_6())
                    .child("No 24/7 agents yet"),
            )
    }
}

impl Render for HomeView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.tab {
            HomeTab::Projects => div()
                .flex()
                .gap(px(24.0))
                .child(self.project_list(cx))
                .child(self.project_detail(cx))
                .into_any_element(),
            HomeTab::Agents => Self::agents().into_any_element(),
        };
        div()
            .id("home")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .justify_center()
            .px(px(theme::SIDE_PAD))
            .py(px(24.0))
            .child(
                div()
                    .w_full()
                    .max_w(px(COLUMN_MAX))
                    .flex()
                    .flex_col()
                    .gap(px(20.0))
                    .child(div().flex().child(self.tab_bar(cx)))
                    .child(body),
            )
    }
}
