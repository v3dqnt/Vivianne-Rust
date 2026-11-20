//! The project rail on the right of a chat: Overview, Files, Preview, Terminal.

mod context;
mod files;
mod git;
pub(crate) mod terminal;

use std::collections::HashSet;
use std::path::PathBuf;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::text::TextView;
use gpui_kit::component::{Icon, IconName, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{ClipboardItem, Context, Entity, SharedString, Window, div, img, px};

pub use self::context::ContextView;
use self::files::Content;
use self::git::{ChangeKind, Snapshot};
use self::terminal::TerminalView;
use crate::theme::{self, color};

/// Width of the rail, and the gap between it and the chat.
pub const WIDTH: f32 = 420.0;
pub const GAP: f32 = 12.0;

/// Longest preview shown, so a big file cannot stall the UI.
const PREVIEW_MAX_LINES: usize = 400;
const INDENT_PX: f32 = 14.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RailTab {
    Overview,
    Files,
    Preview,
    Terminal,
}

const TABS: [(RailTab, &str); 4] = [
    (RailTab::Overview, "Overview"),
    (RailTab::Files, "Files"),
    (RailTab::Preview, "Preview"),
    (RailTab::Terminal, "Terminal"),
];

struct Preview {
    path: PathBuf,
    content: Content,
}

/// What the rail asks the app to do.
#[derive(Debug, Clone, Copy)]
pub enum RailEvent {
    /// Put the project back the way it was before the newest run.
    UndoLastRun,
}

impl gpui_kit::EventEmitter<RailEvent> for Rail {}

pub struct Rail {
    root: PathBuf,
    tab: RailTab,
    expanded: HashSet<PathBuf>,
    preview: Option<Preview>,
    git: Snapshot,
    /// Started the first time the tab is opened, so no shell runs until it is wanted.
    terminal: Option<Entity<TerminalView>>,
    /// The active chat's context window, pushed in by the app; `None` when there is nothing to show.
    context: Option<ContextView>,
    /// What the newest run changed, with the means to undo it.
    changes: Option<vivianne_runtime::checkpoints::LastRun>,
}

impl Rail {
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        let git = Snapshot::read(&root);
        Self { root, tab: RailTab::Overview, expanded: HashSet::new(), preview: None, git, terminal: None, context: None, changes: None }
    }

    fn refresh(&mut self) {
        self.git = Snapshot::read(&self.root);
    }

    fn open_row(&mut self, path: PathBuf, is_dir: bool) {
        if is_dir {
            if !self.expanded.remove(&path) {
                self.expanded.insert(path);
            }
            return;
        }
        self.open_file(path);
    }

    fn open_file(&mut self, path: PathBuf) {
        let content = files::classify(&path);
        self.preview = Some(Preview { path, content });
        self.tab = RailTab::Preview;
    }

    fn show_tab(&mut self, tab: RailTab, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = tab;
        match tab {
            RailTab::Overview | RailTab::Files => self.refresh(),
            RailTab::Terminal => {
                let root = self.root.clone();
                let terminal = self.terminal.get_or_insert_with(|| cx.new(|cx| TerminalView::new(&root, cx)));
                let handle = terminal.read(cx).focus_handle().clone();
                window.focus(&handle, cx);
            }
            RailTab::Preview => {}
        }
        cx.notify();
    }

    fn tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = TABS.iter().position(|(tab, _)| *tab == self.tab).unwrap_or(0);
        TabBar::new("rail-tabs")
            .underline()
            .selected_index(selected)
            .on_click(cx.listener(|rail, index: &usize, window, cx| {
                if let Some((tab, _)) = TABS.get(*index) {
                    rail.show_tab(*tab, window, cx);
                }
            }))
            .children(TABS.iter().map(|(_, label)| Tab::new().label(*label)))
    }

    /// Shows the active chat's context window. Redraws only when it changed.
    pub fn set_context(&mut self, context: Option<ContextView>, cx: &mut Context<Self>) {
        if self.context != context {
            self.context = context;
            cx.notify();
        }
    }

    /// Shows what the newest run changed. Redraws only when it changed.
    pub fn set_changes(&mut self, changes: Option<vivianne_runtime::checkpoints::LastRun>, cx: &mut Context<Self>) {
        if self.changes != changes {
            self.changes = changes;
            cx.notify();
        }
    }

    /// The newest run's files with an Undo button, or nothing when it changed none.
    fn last_run(&self, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        let run = self.changes.as_ref().filter(|run| !run.files.is_empty())?;
        let mut lines = vec![
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(muted(format!("{} {}", run.files.len(), if run.files.len() == 1 { "file" } else { "files" })))
                .child(
                    Button::new("undo-run")
                        .ghost()
                        .small()
                        .label("Undo")
                        .tooltip("Put the project back the way it was before this run")
                        .on_click(cx.listener(|_rail, _event, _window, cx| cx.emit(RailEvent::UndoLastRun))),
                )
                .into_any_element(),
        ];
        lines.extend(run.files.iter().take(12).map(|(path, status)| {
            let hex = match status {
                'A' => theme::OK,
                'D' => theme::DANGER,
                _ => theme::ACCENT,
            };
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(div().w(px(14.0)).flex_none().text_size(px(11.0)).text_color(color(hex)).child(status.to_string()))
                .child(div().min_w_0().overflow_hidden().child(path.clone()))
                .into_any_element()
        }));
        if run.files.len() > 12 {
            lines.push(muted(format!("and {} more", run.files.len() - 12)).into_any_element());
        }
        Some(section("Last run", lines).into_any_element())
    }

    fn overview(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let folder = self.root.file_name().map_or_else(
            || self.root.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(section(
                "Project",
                [
                    div().text_size(px(15.0)).font_weight(gpui_kit::FontWeight::SEMIBOLD).child(folder).into_any_element(),
                    div().font_family(theme::MONO).text_size(px(11.0)).text_color(color(theme::MUTED)).child(self.root.display().to_string()).into_any_element(),
                ],
            ))
            .children(self.context.as_ref().map(context::card))
            .children(self.last_run(cx))
            .child(self.version_control(cx))
    }

    fn version_control(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(branch) = &self.git.branch else {
            return section("Version control", [muted("Not a git repository").into_any_element()]).into_any_element();
        };
        let mut lines = vec![div().flex().items_center().gap(px(8.0)).child(muted("Branch")).child(pill(branch.clone())).into_any_element()];
        if let Some(commit) = &self.git.last_commit {
            lines.push(muted(format!("Last commit: {commit}")).into_any_element());
        }
        let count = self.git.changes.len();
        lines.push(if count == 0 {
            muted("No changes").into_any_element()
        } else {
            muted(format!("{count} changed {}", if count == 1 { "file" } else { "files" })).into_any_element()
        });
        let rows = self.git.changes.iter().enumerate().map(|(index, change)| {
            let path = self.git.top.as_ref().map_or_else(|| PathBuf::from(&change.path), |top| top.join(&change.path));
            div()
                .id(("change", index))
                .flex()
                .items_center()
                .gap(px(8.0))
                .py(px(2.0))
                .rounded(px(theme::RADIUS_CONTROL))
                .cursor_pointer()
                .hover(|style| style.bg(color(theme::RAISED)))
                .on_click(cx.listener(move |rail, _event, _window, cx| {
                    rail.open_file(path.clone());
                    cx.notify();
                }))
                .child(change_mark(change.kind))
                .child(div().min_w_0().overflow_hidden().child(change.path.clone()))
        });
        let refresh = Button::new("refresh-git").label("Refresh").ghost().small().on_click(cx.listener(|rail, _event, _window, cx| {
            rail.refresh();
            cx.notify();
        }));
        section("Version control", lines).child(div().flex().flex_col().children(rows)).child(div().flex().child(refresh)).into_any_element()
    }

    fn files(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let marks = self.git.by_path();
        let rows = files::visible_rows(&self.root, &self.expanded);
        div().flex().flex_col().children(rows.into_iter().enumerate().map(|(index, row)| {
            let icon = match (row.is_dir, row.expanded) {
                (true, true) => IconName::FolderOpen,
                (true, false) => IconName::Folder,
                (false, _) => IconName::File,
            };
            let path = row.path.clone();
            let mark = marks.get(&row.path).map(|kind| change_mark(*kind));
            div()
                .id(("file-row", index))
                .flex()
                .items_center()
                .gap(px(6.0))
                .pl(px(f32::from(u8::try_from(row.depth).unwrap_or(u8::MAX)) * INDENT_PX))
                .py(px(3.0))
                .rounded(px(theme::RADIUS_CONTROL))
                .cursor_pointer()
                .hover(|style| style.bg(color(theme::SURFACE)))
                .on_click(cx.listener(move |rail, _event, _window, cx| {
                    rail.open_row(path.clone(), row.is_dir);
                    cx.notify();
                }))
                .child(Icon::new(icon).size_4().text_color(color(theme::MUTED)))
                .child(div().flex_1().min_w_0().overflow_hidden().child(row.name))
                .children(mark)
        }))
    }

    fn preview(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(preview) = &self.preview else {
            return muted("Open a file from Files to preview it here.").into_any_element();
        };
        let name = preview.path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let copy_path = preview.path.display().to_string();
        let copy = Button::new("copy-path").label("Copy path").ghost().small().on_click(cx.listener(move |_rail, _event, _window, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(copy_path.clone()));
        }));
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(div().flex().items_center().justify_between().child(div().child(name)).child(copy))
            .child(preview_body(preview))
            .into_any_element()
    }
}

/// The file itself: numbered lines for code, rendered markdown, or the image.
fn preview_body(preview: &Preview) -> gpui_kit::AnyElement {
    let frame = || {
        div()
            .id("preview-body")
            .max_h(px(560.0))
            .overflow_y_scroll()
            .p(px(10.0))
            .rounded(px(theme::RADIUS_SURFACE))
            .border_1()
            .border_color(color(theme::BORDER))
            .bg(color(theme::CODE_BG))
    };
    match &preview.content {
        Content::Text(text) => {
            let shown: Vec<&str> = text.lines().take(PREVIEW_MAX_LINES).collect();
            let numbers = (1..=shown.len()).map(|n| n.to_string()).collect::<Vec<_>>().join("\n");
            let code = div().flex_1().min_w_0().child(shown.join("\n"));
            let note = (text.lines().count() > PREVIEW_MAX_LINES).then(|| muted(format!("Showing the first {PREVIEW_MAX_LINES} lines.")));
            frame()
                .font_family(theme::MONO)
                .text_size(px(12.0))
                .child(div().flex().gap(px(12.0)).child(div().text_color(color(theme::MUTED)).child(numbers)).child(code))
                .children(note)
                .into_any_element()
        }
        Content::Markdown(text) => frame()
            .child(TextView::markdown(SharedString::from(format!("preview-{}", preview.path.display())), text.clone()).selectable(true))
            .into_any_element(),
        Content::Image => frame().child(img(preview.path.clone()).max_w_full()).into_any_element(),
        Content::Unavailable(why) => muted(why.clone()).into_any_element(),
    }
}

impl Render for Rail {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match self.tab {
            RailTab::Overview => self.overview(cx).into_any_element(),
            RailTab::Files => self.files(cx).into_any_element(),
            RailTab::Preview => self.preview(cx).into_any_element(),
            RailTab::Terminal => match &self.terminal {
                Some(terminal) => terminal.clone().into_any_element(),
                None => muted("Opening the terminal…").into_any_element(),
            },
        };
        let terminal_tab = self.tab == RailTab::Terminal;
        div()
            .size_full()
            .flex()
            .flex_col()
            .rounded(px(theme::RADIUS_SURFACE))
            .border_1()
            .border_color(color(theme::BORDER))
            .bg(color(theme::PANEL))
            .child(div().px(px(8.0)).child(self.tab_bar(cx)))
            .child(
                div()
                    .id("rail-content")
                    .flex_1()
                    .min_h_0()
                    .when(!terminal_tab, gpui_kit::StatefulInteractiveElement::overflow_y_scroll)
                    .p(px(12.0))
                    .text_size(px(13.0))
                    .child(content),
            )
    }
}

fn change_mark(kind: ChangeKind) -> impl IntoElement {
    let hex = match kind {
        ChangeKind::Added | ChangeKind::Untracked => theme::OK,
        ChangeKind::Deleted => theme::DANGER,
        ChangeKind::Modified | ChangeKind::Renamed => theme::ACCENT,
    };
    div().w(px(14.0)).flex_none().text_size(px(11.0)).text_color(color(hex)).child(kind.letter())
}

/// A titled card: a small spaced label with an accent bar, then its lines.
fn section(title: &str, lines: impl IntoIterator<Item = gpui_kit::AnyElement>) -> gpui_kit::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .p(px(12.0))
        .rounded(px(theme::RADIUS_SURFACE))
        .border_1()
        .border_color(color(theme::BORDER))
        .bg(color(theme::SURFACE))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(7.0))
                .child(div().w(px(3.0)).h(px(11.0)).flex_none().rounded_full().bg(color(theme::ACCENT)))
                .child(div().text_size(px(11.0)).text_color(color(theme::MUTED)).child(title.to_uppercase())),
        )
        .children(lines)
}

/// A short value shown as a pill: the branch name, a count.
fn pill(text: String) -> gpui_kit::Div {
    div()
        .px(px(8.0))
        .py(px(2.0))
        .rounded_full()
        .bg(color(theme::ACCENT_SOFT))
        .border_1()
        .border_color(color(theme::ACCENT_LINE))
        .text_size(px(12.0))
        .text_color(color(theme::ACCENT_TEXT))
        .child(text)
}

fn muted(text: impl Into<SharedString>) -> gpui_kit::Div {
    div().text_color(color(theme::MUTED)).child(text.into())
}
