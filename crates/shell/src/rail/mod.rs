//! The project rail on the right of a chat: Overview, Files, Preview, Terminal.

mod context;
mod debugger;
mod file_icons;
mod files;
mod git;
pub mod performance;
pub(crate) mod terminal;

use std::collections::HashSet;
use std::path::PathBuf;

use gpui_kit::base::input::EditorState;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Editor;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::text::TextView;
use gpui_kit::component::{Icon, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{ClipboardItem, Context, Entity, SharedString, Window, div, img, px};

pub use self::context::ContextView;
use self::debugger::DebuggerView;
use self::files::Content;
use self::git::{ChangeKind, Snapshot};
use self::terminal::TerminalView;
use crate::theme::{self, color};

/// Width of the rail.
pub const WIDTH: f32 = 380.0;

const INDENT_PX: f32 = 14.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RailTab {
    Overview,
    Files,
    Preview,
    Terminal,
    Debug,
}

impl RailTab {
    /// The name it is remembered by.
    fn name(self) -> &'static str {
        match self {
            Self::Overview => "overview",
            Self::Files => "files",
            Self::Preview => "preview",
            Self::Terminal => "terminal",
            Self::Debug => "debug",
        }
    }

    /// The tab remembered from last time. The terminal and the preview are not reopened by themselves: a shell
    /// should not start unasked, and a preview has no file until one is chosen.
    fn remembered() -> Self {
        match crate::ui_state::load().rail_tab.as_deref() {
            Some("files") => Self::Files,
            _ => Self::Overview,
        }
    }
}

const TABS: [(RailTab, &str); 5] = [
    (RailTab::Overview, "Overview"),
    (RailTab::Files, "Files"),
    (RailTab::Preview, "Preview"),
    (RailTab::Terminal, "Terminal"),
    (RailTab::Debug, "Debug"),
];

struct Preview {
    path: PathBuf,
    content: Content,
    /// A text file is shown in a read-only editor: syntax colours, line numbers, wrapping, selection.
    editor: Option<Entity<EditorState>>,
}

/// What the rail asks the app to do.
#[derive(Debug, Clone, Copy)]
pub enum RailEvent {
    /// Put the project back the way it was before the newest run.
    UndoLastRun,
    /// Show the repository on a page of its own.
    OpenGit,
    /// Show the page for working with a partner.
    OpenPair,
}

impl gpui_kit::EventEmitter<RailEvent> for Rail {}

pub struct Rail {
    root: PathBuf,
    tab: RailTab,
    /// Where the selection pill was before the last tab change, and a count that starts its move: the pill
    /// stretches toward the new tab and settles into it.
    pill_from: usize,
    pill_moves: u32,
    /// Overview sections the person has minimised.
    collapsed: HashSet<String>,
    expanded: HashSet<PathBuf>,
    preview: Option<Preview>,
    git: Snapshot,
    /// Started the first time the tab is opened, so no shell runs until it is wanted.
    terminal: Option<Entity<TerminalView>>,
    /// Started the first time the tab is opened; it reads the project's debug session while it exists.
    debug: Option<Entity<DebuggerView>>,
    /// The active chat's context window, pushed in by the app; `None` when there is nothing to show.
    context: Option<ContextView>,
    /// What the newest run changed, with the means to undo it.
    changes: Option<vivianne_runtime::checkpoints::LastRun>,
    /// The editors on this machine, looked up once.
    open_apps: Vec<vivianne_tools::open_apps::OpenApp>,
    /// How the project's dev server starts, when it has one.
    dev_command: Option<String>,
    /// Where the repository stands beyond what this rail reads itself: unpushed commits, the pull request.
    badge: Option<crate::git::Badge>,
    /// The partner this project is paired with, by name.
    partner: Option<String>,
}

impl Rail {
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        let git = Snapshot::read(&root);
        let dev_command = vivianne_tools::dev_server::detect_command(&root);
        let mut rail = Self {
            root,
            tab: RailTab::remembered(),
            pill_from: 0,
            pill_moves: 0,
            collapsed: crate::ui_state::load().rail_collapsed.into_iter().collect(),
            expanded: HashSet::new(),
            preview: None,
            git,
            terminal: None,
            debug: None,
            context: None,
            changes: None,
            open_apps: vivianne_tools::open_apps::list(),
            dev_command,
            badge: None,
            partner: None,
        };
        rail.refresh();
        rail
    }

    fn refresh(&mut self) {
        self.git = Snapshot::read(&self.root);
        self.partner = vivianne_runtime::projects::paired_at(&self.root).map(|p| {
            p.pairing()
                .and_then(|m| m.peer_display_name)
                .unwrap_or_else(|| "a partner".to_owned())
        });
    }

    fn open_row(
        &mut self,
        path: PathBuf,
        is_dir: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if is_dir {
            if !self.expanded.remove(&path) {
                self.expanded.insert(path);
            }
            return;
        }
        self.open_file(path, window, cx);
    }

    fn open_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let content = files::classify(&path);
        let editor = match &content {
            Content::Text(text) => {
                // Syntect knows a language by its extension or, for `Dockerfile` and the like, its name.
                let language = path
                    .extension()
                    .or_else(|| path.file_name())
                    .map(|name| name.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                let text = text.clone();
                let state = cx.new(|cx| {
                    EditorState::new(window, cx)
                        .language(language)
                        .line_number(true)
                        .soft_wrap(true)
                        .default_value(text)
                });
                state.update(cx, |state, cx| state.set_readonly(true, cx));
                Some(state)
            }
            _ => None,
        };
        self.preview = Some(Preview {
            path,
            content,
            editor,
        });
        self.tab = RailTab::Preview;
        cx.notify();
    }

    fn show_tab(&mut self, tab: RailTab, window: &mut Window, cx: &mut Context<Self>) {
        if tab != self.tab {
            self.pill_from = TABS.iter().position(|(t, _)| *t == self.tab).unwrap_or(0);
            self.pill_moves += 1;
        }
        self.tab = tab;
        crate::ui_state::save_rail_tab(tab.name());
        match tab {
            RailTab::Overview | RailTab::Files => self.refresh(),
            RailTab::Terminal => {
                let root = self.root.clone();
                let terminal = self
                    .terminal
                    .get_or_insert_with(|| cx.new(|cx| TerminalView::new(&root, cx)));
                let handle = terminal.read(cx).focus_handle().clone();
                window.focus(&handle, cx);
            }
            RailTab::Debug => {
                let root = self.root.clone();
                self.debug
                    .get_or_insert_with(|| cx.new(|cx| DebuggerView::new(&root, cx)));
            }
            RailTab::Preview => {}
        }
        cx.notify();
    }

    /// The tabs: an iOS-style segmented control. The selected segment is a pill that slides to the new tab with
    /// squash and stretch: its leading edge races ahead while the trailing edge lags (it stretches), and it is a
    /// little shorter while it travels (it squashes), then settles to its size.
    fn tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui_kit::{Animation, AnimationExt as _, relative};
        let count = TABS.len() as f32;
        let to = TABS.iter().position(|(t, _)| *t == self.tab).unwrap_or(0);
        let from = self.pill_from.min(TABS.len() - 1);
        let pill = div()
            .absolute()
            .rounded(px(7.0))
            .bg(theme::glass_lift(0x2e))
            .with_animation(
                ("rail-pill", self.pill_moves),
                Animation::new(std::time::Duration::from_millis(220)),
                move |pill, t| {
                    let (from_l, to_l) = (from as f32 / count, to as f32 / count);
                    let (from_r, to_r) = ((from as f32 + 1.0) / count, (to as f32 + 1.0) / count);
                    // The edge that leads moves fast and eases out; the one behind starts late and eases in.
                    let lead = 1.0 - (1.0 - t).powi(4);
                    let lag = t * t * (3.0 - 2.0 * t);
                    let lag = lag * lag.sqrt().max(0.0);
                    let (left, right) = if to >= from {
                        (from_l + (to_l - from_l) * lag, from_r + (to_r - from_r) * lead)
                    } else {
                        (from_l + (to_l - from_l) * lead, from_r + (to_r - from_r) * lag)
                    };
                    // Shorter at the middle of the move, full height at the ends.
                    let squash = if from == to { 0.0 } else { 5.0 * (t * std::f32::consts::PI).sin() };
                    pill.left(relative(left))
                        .w(relative((right - left).max(0.0)))
                        .top(px(squash / 2.0))
                        .h(px(28.0 - squash))
                },
            );
        div()
            .flex()
            .w_full()
            .items_center()
            .p(px(2.0))
            .mt(px(10.0))
            .mb(px(6.0))
            .rounded(px(9.0))
            .bg(theme::glass_lift(0x14))
            .child(
                div()
                    .relative()
                    .flex()
                    .w_full()
                    .child(pill)
                    .children(TABS.iter().enumerate().map(|(index, (tab, label))| {
                        let (tab, selected) = (*tab, self.tab == *tab);
                        div()
                            .id(("rail-tab", index))
                            .h(px(28.0))
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(7.0))
                            .cursor_pointer()
                            .text_size(px(theme::TEXT_SM))
                            .whitespace_nowrap()
                            .font_weight(if selected {
                                gpui_kit::FontWeight::SEMIBOLD
                            } else {
                                gpui_kit::FontWeight::MEDIUM
                            })
                            .text_color(color(if selected { theme::TEXT } else { theme::MUTED }))
                            .when(!selected, |segment| {
                                segment.hover(|style| style.text_color(color(theme::TEXT)))
                            })
                            .on_click(cx.listener(move |rail, _event, window, cx| {
                                rail.show_tab(tab, window, cx);
                            }))
                            .child(*label)
                    })),
            )
    }

    /// Shows the active chat's context window. Redraws only when it changed.
    pub fn set_context(&mut self, context: Option<ContextView>, cx: &mut Context<Self>) {
        if self.context != context {
            self.context = context;
            cx.notify();
        }
    }

    /// Shows the repository's standing from the Git tab. Redraws only when it changed.
    pub fn set_badge(&mut self, badge: Option<crate::git::Badge>, cx: &mut Context<Self>) {
        if self.badge != badge {
            self.badge = badge;
            cx.notify();
        }
    }

    /// Shows what the newest run changed. Redraws only when it changed.
    pub fn set_changes(
        &mut self,
        changes: Option<vivianne_runtime::checkpoints::LastRun>,
        cx: &mut Context<Self>,
    ) {
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
                .child(
                    div()
                        .w(px(14.0))
                        .flex_none()
                        .font_family(theme::MONO)
                        .text_size(px(theme::TEXT_XS))
                        .text_color(color(hex))
                        .child(status.to_string()),
                )
                .child(div().min_w_0().truncate().child(path.clone()))
                .into_any_element()
        }));
        if run.files.len() > 12 {
            lines.push(muted(format!("and {} more", run.files.len() - 12)).into_any_element());
        }
        let title = format!(
            "Last run · {} {}",
            run.files.len(),
            if run.files.len() == 1 {
                "file"
            } else {
                "files"
            }
        );
        Some(section_with(&title, Some(undo.into_any_element()), lines).into_any_element())
    }

    /// Start or stop the project's dev server, and watch it until it has said where it is or has failed.
    fn toggle_dev_server(&mut self, cx: &mut Context<Self>) {
        use vivianne_tools::dev_server::{DEV_SERVERS, Status};
        let id = self.root.display().to_string();
        if DEV_SERVERS.status(&id) == Status::Stopped
            || matches!(DEV_SERVERS.status(&id), Status::Crashed { .. })
        {
            let Some(command) = self.dev_command.clone() else {
                return;
            };
            // A server that cannot start shows as crashed below, so there is nothing more to say here.
            let _ = crate::backend::start_dev_server(&id, &id, &command);
            cx.spawn(async move |rail, cx| {
                // What it prints arrives on its own time, so look every half second until it is settled.
                while DEV_SERVERS.status(&id) == Status::Starting {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(500))
                        .await;
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
            div()
                .font_family(theme::MONO)
                .text_size(px(theme::TEXT_SM))
                .child(command)
                .into_any_element(),
            muted(state).into_any_element(),
        ];
        if let Status::Running { url } = status {
            let open = url.clone();
            lines.push(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .font_family(theme::MONO)
                            .text_size(px(theme::TEXT_SM))
                            .text_color(color(theme::ACCENT_TEXT))
                            .child(url),
                    )
                    .child(
                        Button::new("dev-open")
                            .ghost()
                            .xsmall()
                            .label("Open")
                            .on_click(
                                cx.listener(move |_rail, _event, _window, cx| cx.open_url(&open)),
                            ),
                    )
                    .into_any_element(),
            );
        }
        Some(section_with("Dev server", Some(switch.into_any_element()), lines).into_any_element())
    }

    /// Minimises a section of the Overview, or opens it again, and remembers which.
    fn toggle_section(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.collapsed.remove(id) {
            self.collapsed.insert(id.to_owned());
        }
        let mut all: Vec<String> = self.collapsed.iter().cloned().collect();
        all.sort();
        crate::ui_state::save_rail_collapsed(all);
        cx.notify();
    }

    fn overview(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let context_closed = self.collapsed.contains("context");
        div()
            .flex()
            .flex_col()
            // The first section's top rule would sit right under the tabs: it is clipped away.
            .mt(px(-1.0))
            .children(self.context.as_ref().map(|view| {
                context::card(
                    view,
                    context_closed,
                    cx.listener(|rail, _e, _w, cx| rail.toggle_section("context", cx)),
                )
            }))
            .children(self.last_run(cx))
            .children(self.dev_server(cx))
            .child(self.version_control(cx))
            .child(self.pair(cx))
    }

    /// Working with a partner: who the project is paired with, and the way to the page.
    fn pair(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let open = Button::new("open-pair")
            .label(if self.partner.is_some() {
                "Open"
            } else {
                "Set up"
            })
            .ghost()
            .xsmall()
            .on_click(cx.listener(|_rail, _event, _window, cx| cx.emit(RailEvent::OpenPair)));
        let line = match &self.partner {
            Some(name) => format!("Paired with {name}"),
            None => "Work on this project with someone on another machine".to_owned(),
        };
        section_with(
            "Pair",
            Some(open.into_any_element()),
            [muted(line).into_any_element()],
        )
    }

    fn version_control(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(branch) = &self.git.branch else {
            return section(
                "Version control",
                [muted("Not a git repository").into_any_element()],
            )
            .into_any_element();
        };
        let mut lines = vec![field(
            "Branch",
            div()
                .font_family(theme::MONO)
                .text_size(px(theme::TEXT_SM))
                .child(branch.clone()),
        )];
        if let Some(commit) = &self.git.last_commit {
            // "8eca991 Offer Vivianne's own tools..." reads as a hash, then the message.
            let (hash, message) = commit.split_once(' ').unwrap_or((commit.as_str(), ""));
            lines.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_size(px(theme::TEXT_XS))
                            .text_color(color(theme::FAINT))
                            .child("Last commit"),
                    )
                    .child(
                        div()
                            .text_color(color(theme::TEXT))
                            .child(message.to_owned()),
                    )
                    .child(
                        div()
                            .font_family(theme::MONO)
                            .text_size(px(theme::TEXT_XS))
                            .text_color(color(theme::FAINT))
                            .child(hash.to_owned()),
                    )
                    .into_any_element(),
            );
        }
        if let Some(badge) = self.badge {
            let mut parts = Vec::new();
            if badge.ahead > 0 {
                parts.push(format!("{} to push", badge.ahead));
            }
            if badge.behind > 0 {
                parts.push(format!("{} to pull", badge.behind));
            }
            if let Some((number, checks)) = badge.pr {
                parts.push(if checks.is_empty() {
                    format!("PR #{number}")
                } else {
                    format!("PR #{number}, {checks}")
                });
            }
            if !parts.is_empty() {
                let hex = if badge.attention == Some(vivianne_tools::git::Attention::Red) {
                    theme::DANGER
                } else {
                    theme::MUTED
                };
                lines.push(
                    div()
                        .text_color(color(hex))
                        .child(parts.join(" · "))
                        .into_any_element(),
                );
            }
        }
        let count = self.git.changes.len();
        lines.push(if count == 0 {
            muted("No changes").into_any_element()
        } else {
            muted(format!(
                "{count} changed {}",
                if count == 1 { "file" } else { "files" }
            ))
            .into_any_element()
        });
        // A long list of changes is the version control tool's job; here, the first few and how many more.
        const SHOWN: usize = 8;
        let more = self.git.changes.len().saturating_sub(SHOWN);
        let rows = self
            .git
            .changes
            .iter()
            .take(SHOWN)
            .enumerate()
            .map(|(index, change)| {
                let path = self
                    .git
                    .top
                    .as_ref()
                    .map_or_else(|| PathBuf::from(&change.path), |top| top.join(&change.path));
                div()
                    .id(("change", index))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .py(px(2.0))
                    .rounded(px(theme::RADIUS_CONTROL))
                    .cursor_pointer()
                    .hover(|style| style.bg(color(theme::RAISED)))
                    .on_click(cx.listener(move |rail, _event, window, cx| {
                        rail.open_file(path.clone(), window, cx);
                        cx.notify();
                    }))
                    .child(change_mark(change.kind))
                    .child(div().min_w_0().overflow_hidden().child(change.path.clone()))
            });
        let refresh = Button::new("refresh-git")
            .label("Refresh")
            .ghost()
            .xsmall()
            .on_click(cx.listener(|rail, _event, _window, cx| {
                rail.refresh();
                cx.notify();
            }));
        let open = Button::new("open-git")
            .label("Open")
            .ghost()
            .xsmall()
            .tooltip("The whole repository: changes, pull requests, Actions, issues, history")
            .on_click(cx.listener(|_rail, _event, _window, cx| cx.emit(RailEvent::OpenGit)));
        let closed = self.collapsed.contains("version-control");
        let right = if closed {
            // Minimised: the branch and how many files changed, so the line still says where things stand.
            let summary = if count == 0 {
                format!("{branch} · clean")
            } else {
                format!("{branch} · {count} changed")
            };
            muted(summary).into_any_element()
        } else {
            div()
                .flex()
                .items_center()
                .gap(px(2.0))
                .child(refresh)
                .child(open)
                .into_any_element()
        };
        let section = collapsible(
            "version-control",
            "Version control",
            Some(right),
            closed,
            cx.listener(|rail, _e, _w, cx| rail.toggle_section("version-control", cx)),
            if closed { Vec::new() } else { lines },
        );
        if closed {
            return section.into_any_element();
        }
        section
            .child(div().flex().flex_col().children(rows))
            .when(more > 0, |section| {
                section.child(muted(format!("and {more} more")))
            })
            .into_any_element()
    }

    fn files(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let marks = self.git.by_path();
        let rows = files::visible_rows(&self.root, &self.expanded);
        div()
            .flex()
            .flex_col()
            .children(rows.into_iter().enumerate().map(|(index, row)| {
                let look = if row.is_dir {
                    file_icons::folder(&row.name, row.expanded)
                } else {
                    file_icons::file(&row.name)
                };
                let path = row.path.clone();
                let mark = marks.get(&row.path).map(|kind| change_mark(*kind));
                div()
                    .id(("file-row", index))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .pl(px(
                        f32::from(u8::try_from(row.depth).unwrap_or(u8::MAX)) * INDENT_PX
                    ))
                    .py(px(3.0))
                    .rounded(px(theme::RADIUS_CONTROL))
                    .cursor_pointer()
                    .hover(|style| style.bg(color(theme::SURFACE)))
                    .on_click(cx.listener(move |rail, _event, window, cx| {
                        rail.open_row(path.clone(), row.is_dir, window, cx);
                        cx.notify();
                    }))
                    .child(
                        Icon::empty()
                            .path(look.icon)
                            .size_4()
                            .text_color(color(look.color)),
                    )
                    .child(div().flex_1().min_w_0().overflow_hidden().child(row.name))
                    .children(mark)
            }))
    }

    /// The file fills the tab: a bar with its name and what can be done with it, then the file itself.
    fn preview(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let Some(preview) = &self.preview else {
            return div()
                .p(px(20.0))
                .child(muted("Open a file from Files to preview it here."))
                .into_any_element();
        };
        let name = preview
            .path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let look = file_icons::file(&name);
        let copy_path = preview.path.display().to_string();
        let open_path = copy_path.clone();
        let apps = self.open_apps.clone();
        let open_in = Button::new("open-in")
            .label("Open in")
            .ghost()
            .xsmall()
            .dropdown_caret(true)
            .dropdown_menu_with_anchor(gpui_kit::Anchor::BottomRight, move |menu, _window, _cx| {
                apps.iter().fold(menu, |menu, app| {
                    let (id, path) = (app.id.clone(), open_path.clone());
                    menu.item(PopupMenuItem::new(app.label.clone()).on_click(
                        move |_event, _window, _app| {
                            // A launcher that fails to start has nowhere to say so here; the file simply does not open.
                            let _ = vivianne_tools::open_apps::open_in(&path, &id, None);
                        },
                    ))
                })
            });
        let copy = Button::new("copy-path")
            .label("Copy path")
            .ghost()
            .xsmall()
            .on_click(cx.listener(move |_rail, _event, _window, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(copy_path.clone()));
            }));
        let bar = div()
            .h(px(40.0))
            .flex_none()
            .px(px(14.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .border_b_1()
            .border_color(color(theme::HAIRLINE))
            .child(
                Icon::empty()
                    .path(look.icon)
                    .size_4()
                    .flex_none()
                    .text_color(color(look.color)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child(name),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(2.0))
                    .child(open_in)
                    .child(copy),
            );
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(bar)
            .child(div().flex_1().min_h_0().child(preview_body(preview)))
            .into_any_element()
    }
}

/// The file itself, using the whole space under the bar: code with colours and line numbers, rendered markdown,
/// or the image.
fn preview_body(preview: &Preview) -> gpui_kit::AnyElement {
    let page = |id: &'static str| {
        div()
            .id(id)
            .size_full()
            .overflow_y_scroll()
            .px(px(20.0))
            .py(px(16.0))
    };
    match (&preview.content, &preview.editor) {
        (Content::Text(_), Some(editor)) =>
        // This is the component-layer editor: it is what installs the syntax highlighter.
        {
            div()
                .size_full()
                .child(
                    Editor::new(editor)
                        .appearance(false)
                        .readonly(true)
                        .h(gpui_kit::relative(1.0)),
                )
                .into_any_element()
        }
        (Content::Markdown(text), _) => page("preview-markdown")
            .child(
                TextView::markdown(
                    SharedString::from(format!("preview-{}", preview.path.display())),
                    text.clone(),
                )
                .selectable(true),
            )
            .into_any_element(),
        (Content::Image, _) => page("preview-image")
            .child(img(preview.path.clone()).max_w_full())
            .into_any_element(),
        (Content::Unavailable(why), _) => page("preview-note")
            .child(muted(why.clone()))
            .into_any_element(),
        (Content::Text(_), None) => page("preview-note")
            .child(muted("Nothing to show."))
            .into_any_element(),
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
            RailTab::Debug => match &self.debug {
                Some(debug) => debug.clone().into_any_element(),
                None => muted("Opening the debug pane…").into_any_element(),
            },
        };
        // The terminal and the preview fill the tab and scroll themselves; the rest is a padded page.
        let terminal_tab = matches!(
            self.tab,
            RailTab::Terminal | RailTab::Preview | RailTab::Debug
        );
        div()
            .size_full()
            .flex()
            .flex_col()
            // Same surface as the header bar and the chat: no divider, no panel colour.
            .bg(theme::glass())
            .overflow_hidden()
            .child(div().px(px(12.0)).child(self.tab_bar(cx)))
            .child(
                div()
                    .id("rail-content")
                    .flex_1()
                    .min_h_0()
                    .when(
                        !terminal_tab,
                        gpui_kit::StatefulInteractiveElement::overflow_y_scroll,
                    )
                    .px(px(if terminal_tab { 0.0 } else { 20.0 }))
                    .pb(px(if terminal_tab { 0.0 } else { 8.0 }))
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
    div()
        .w(px(14.0))
        .flex_none()
        .text_size(px(11.0))
        .text_color(color(hex))
        .child(kind.letter())
}

/// A section of the panel: a title in the panel's own voice (sentence case, no decoration), then its lines.
/// Sections are told apart by space and a hairline between them, not by boxes.
fn section(title: &str, lines: impl IntoIterator<Item = gpui_kit::AnyElement>) -> gpui_kit::Div {
    section_with(title, None, lines)
}

/// A section with an action (a button) at the right of its title.
fn section_with(
    title: &str,
    action: Option<gpui_kit::AnyElement>,
    lines: impl IntoIterator<Item = gpui_kit::AnyElement>,
) -> gpui_kit::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .py(px(16.0))
        .border_t_1()
        .border_color(color(theme::HAIRLINE))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .h(px(22.0))
                .child(
                    div()
                        .text_size(px(theme::TEXT_SM))
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .child(title.to_owned()),
                )
                .children(action),
        )
        .children(lines)
}

/// A section whose title can be clicked to minimise it: a chevron and the title at the left, `right` at the right
/// (always shown), and the lines below unless it is minimised.
fn collapsible(
    id: &'static str,
    title: &str,
    right: Option<gpui_kit::AnyElement>,
    closed: bool,
    on_toggle: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
    lines: Vec<gpui_kit::AnyElement>,
) -> gpui_kit::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .py(px(if closed { 10.0 } else { 16.0 }))
        .border_t_1()
        .border_color(color(theme::HAIRLINE))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .h(px(22.0))
                .child(
                    div()
                        .id(id)
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .cursor_pointer()
                        .on_click(on_toggle)
                        .child(
                            Icon::new(if closed {
                                gpui_kit::component::IconName::ChevronRight
                            } else {
                                gpui_kit::component::IconName::ChevronDown
                            })
                            .size_4()
                            .text_color(color(theme::MUTED)),
                        )
                        .child(
                            div()
                                .text_size(px(theme::TEXT_SM))
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .child(title.to_owned()),
                        ),
                )
                .children(right),
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
        .child(
            div()
                .text_color(color(theme::MUTED))
                .child(label.to_owned()),
        )
        .child(value)
        .into_any_element()
}

fn muted(text: impl Into<SharedString>) -> gpui_kit::Div {
    div().text_color(color(theme::MUTED)).child(text.into())
}
