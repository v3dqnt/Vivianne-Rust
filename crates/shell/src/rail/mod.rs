//! The project rail on the right of a chat: Overview, Files, Preview, Terminal.

mod context;
mod files;
mod git;
pub(crate) mod terminal;

use std::collections::HashSet;
use std::path::PathBuf;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
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

/// Width of the rail, and the space around it.
pub const WIDTH: f32 = 380.0;
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
    /// The editors on this machine, looked up once.
    open_apps: Vec<vivianne_tools::open_apps::OpenApp>,
    /// How the project's dev server starts, when it has one.
    dev_command: Option<String>,
}

impl Rail {
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        let git = Snapshot::read(&root);
        let dev_command = vivianne_tools::dev_server::detect_command(&root);
        Self { root, tab: RailTab::Overview, expanded: HashSet::new(), preview: None, git, terminal: None, context: None, changes: None, open_apps: vivianne_tools::open_apps::list(), dev_command }
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
            .segmented()
            .small()
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
        let undo = Button::new("undo-run")
            .ghost()
            .xsmall()
            .label("Undo")
            .tooltip("Put the project back the way it was before this run")
            .on_click(cx.listener(|_rail, _event, _window, cx| cx.emit(RailEvent::UndoLastRun)));
        let mut lines = Vec::new();
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
                .child(div().w(px(14.0)).flex_none().font_family(theme::MONO).text_size(px(theme::TEXT_XS)).text_color(color(hex)).child(status.to_string()))
                .child(div().min_w_0().truncate().child(path.clone()))
                .into_any_element()
        }));
        if run.files.len() > 12 {
            lines.push(muted(format!("and {} more", run.files.len() - 12)).into_any_element());
        }
        let title = format!("Last run · {} {}", run.files.len(), if run.files.len() == 1 { "file" } else { "files" });
        Some(section_with(&title, Some(undo.into_any_element()), lines).into_any_element())
    }

    /// Start or stop the project's dev server, and watch it until it has said where it is or has failed.
    fn toggle_dev_server(&mut self, cx: &mut Context<Self>) {
        use vivianne_tools::dev_server::{DEV_SERVERS, Status};
        let id = self.root.display().to_string();
        if DEV_SERVERS.status(&id) == Status::Stopped || matches!(DEV_SERVERS.status(&id), Status::Crashed { .. }) {
            let Some(command) = self.dev_command.clone() else { return };
            // A server that cannot start shows as crashed below, so there is nothing more to say here.
            let _ = crate::backend::start_dev_server(&id, &id, &command);
            cx.spawn(async move |rail, cx| {
                // What it prints arrives on its own time, so look every half second until it is settled.
                while DEV_SERVERS.status(&id) == Status::Starting {
                    cx.background_executor().timer(std::time::Duration::from_millis(500)).await;
                    if rail.update(cx, |_, cx| cx.notify()).is_err() {
                        return;
                    }
                }
                rail.update(cx, |_, cx| cx.notify()).ok();
            })
            .detach();
        } else {
            DEV_SERVERS.stop(&id);
        }
        cx.notify();
    }

    /// The dev server card: the command, what state it is in, and the address once it has printed one.
    fn dev_server(&self, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        use vivianne_tools::dev_server::{DEV_SERVERS, Status};
        let command = self.dev_command.clone()?;
        let status = DEV_SERVERS.status(&self.root.display().to_string());
        let (state, running) = match &status {
            Status::Stopped => ("Stopped".to_owned(), false),
            Status::Starting => ("Starting…".to_owned(), true),
            Status::Running { .. } => ("Running".to_owned(), true),
            Status::Crashed { error } => (error.clone(), false),
        };
        let switch = Button::new("dev-server")
            .ghost()
            .xsmall()
            .label(if running { "Stop" } else { "Start" })
            .on_click(cx.listener(|rail, _event, _window, cx| rail.toggle_dev_server(cx)));
        let mut lines = vec![
            div().font_family(theme::MONO).text_size(px(theme::TEXT_SM)).child(command).into_any_element(),
            muted(state).into_any_element(),
        ];
        if let Status::Running { url } = status {
            let open = url.clone();
            lines.push(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().font_family(theme::MONO).text_size(px(theme::TEXT_SM)).text_color(color(theme::ACCENT_TEXT)).child(url))
                    .child(Button::new("dev-open").ghost().xsmall().label("Open").on_click(cx.listener(move |_rail, _event, _window, cx| cx.open_url(&open))))
                    .into_any_element(),
            );
        }
        Some(section_with("Dev server", Some(switch.into_any_element()), lines).into_any_element())
    }

    fn overview(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .children(self.context.as_ref().map(context::card))
            .children(self.last_run(cx))
            .children(self.dev_server(cx))
            .child(self.version_control(cx))
    }

    fn version_control(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(branch) = &self.git.branch else {
            return section("Version control", [muted("Not a git repository").into_any_element()]).into_any_element();
        };
        let mut lines = vec![field("Branch", div().font_family(theme::MONO).text_size(px(theme::TEXT_SM)).child(branch.clone()))];
        if let Some(commit) = &self.git.last_commit {
            // "8eca991 Offer Vivianne's own tools..." reads as a hash, then the message.
            let (hash, message) = commit.split_once(' ').unwrap_or((commit.as_str(), ""));
            lines.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(div().text_size(px(theme::TEXT_XS)).text_color(color(theme::FAINT)).child("Last commit"))
                    .child(div().text_color(color(theme::TEXT)).child(message.to_owned()))
                    .child(div().font_family(theme::MONO).text_size(px(theme::TEXT_XS)).text_color(color(theme::FAINT)).child(hash.to_owned()))
                    .into_any_element(),
            );
        }
        let count = self.git.changes.len();
        lines.push(if count == 0 {
            muted("No changes").into_any_element()
        } else {
            muted(format!("{count} changed {}", if count == 1 { "file" } else { "files" })).into_any_element()
        });
        // A long list of changes is the version control tool's job; here, the first few and how many more.
        const SHOWN: usize = 8;
        let more = self.git.changes.len().saturating_sub(SHOWN);
        let rows = self.git.changes.iter().take(SHOWN).enumerate().map(|(index, change)| {
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
        let refresh = Button::new("refresh-git").label("Refresh").ghost().xsmall().on_click(cx.listener(|rail, _event, _window, cx| {
            rail.refresh();
            cx.notify();
        }));
        section_with("Version control", Some(refresh.into_any_element()), lines)
            .child(div().flex().flex_col().children(rows))
            .when(more > 0, |section| section.child(muted(format!("and {more} more"))))
            .into_any_element()
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
        let open_path = copy_path.clone();
        let apps = self.open_apps.clone();
        let open_in = Button::new("open-in").label("Open in").ghost().small().dropdown_caret(true).dropdown_menu_with_anchor(gpui_kit::Anchor::BottomRight, move |menu, _window, _cx| {
            apps.iter().fold(menu, |menu, app| {
                let (id, path) = (app.id.clone(), open_path.clone());
                menu.item(PopupMenuItem::new(app.label.clone()).on_click(move |_event, _window, _app| {
                    // A launcher that fails to start has nowhere to say so here; the file simply does not open.
                    let _ = vivianne_tools::open_apps::open_in(&path, &id, None);
                }))
            })
        });
        let copy = Button::new("copy-path").label("Copy path").ghost().small().on_click(cx.listener(move |_rail, _event, _window, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(copy_path.clone()));
        }));
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(div().flex().items_center().justify_between().child(div().child(name)).child(div().flex().gap(px(4.0)).child(open_in).child(copy)))
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
            .rounded(px(theme::RADIUS_PANEL))
            .border_1()
            .border_color(color(theme::HAIRLINE))
            .bg(color(theme::PANEL))
            .overflow_hidden()
            .child(div().p(px(10.0)).child(self.tab_bar(cx)))
            .child(
                div()
                    .id("rail-content")
                    .flex_1()
                    .min_h_0()
                    .when(!terminal_tab, gpui_kit::StatefulInteractiveElement::overflow_y_scroll)
                    .px(px(18.0))
                    .py(px(if terminal_tab { 0.0 } else { 4.0 }))
                    .text_size(px(theme::TEXT_SM + 0.5))
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

/// A section of the panel: a title in the panel's own voice (sentence case, no decoration), then its lines.
/// Sections are told apart by space and a hairline between them, not by boxes.
fn section(title: &str, lines: impl IntoIterator<Item = gpui_kit::AnyElement>) -> gpui_kit::Div {
    section_with(title, None, lines)
}

/// A section with an action (a button) at the right of its title.
fn section_with(title: &str, action: Option<gpui_kit::AnyElement>, lines: impl IntoIterator<Item = gpui_kit::AnyElement>) -> gpui_kit::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .py(px(16.0))
        .border_b_1()
        .border_color(color(theme::HAIRLINE))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .h(px(22.0))
                .child(div().text_size(px(theme::TEXT_SM)).font_weight(gpui_kit::FontWeight::SEMIBOLD).child(title.to_owned()))
                .children(action),
        )
        .children(lines)
}

/// A label and its value on one line, the label quiet.
fn field(label: &str, value: impl IntoElement) -> gpui_kit::AnyElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.0))
        .child(div().text_color(color(theme::MUTED)).child(label.to_owned()))
        .child(value)
        .into_any_element()
}

fn muted(text: impl Into<SharedString>) -> gpui_kit::Div {
    div().text_color(color(theme::MUTED)).child(text.into())
}
