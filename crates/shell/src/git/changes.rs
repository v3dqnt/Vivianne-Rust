//! The Changes tab: every changed file in the project with its diff, unstaged or staged, laid out unified or
//! side by side. The list is read again every few seconds while the tab is open.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::time::Duration;

use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::text::TextView;
use gpui_kit::prelude::*;
use gpui_kit::{
    Anchor, ClipboardItem, Context, FontWeight, HighlightStyle, Hsla, SharedString, StyledText,
    Window, div, px, rgb,
};
use vivianne_tools::git::FileChange;
use vivianne_tools::git::diff::{self, Kind, Line, Seg};

use super::ui::muted;
use crate::theme::{self, color};

/// Context large enough that git shows the whole file around a change.
const WHOLE_FILE: u32 = 100_000;
/// Wide enough for two columns of code side by side.
const SPLIT_FROM_PX: f32 = 980.0;
const POLL: Duration = Duration::from_secs(4);
/// A file with more lines than this shows its first lines and says so, so one huge diff cannot stall the window.
const MAX_LINES: usize = 1500;
const LINE_HEIGHT: f32 = 18.0;
const ADD_BG: u32 = 0x0016_3d2a;
const DEL_BG: u32 = 0x0041_1f22;
const ADD_WORD: u32 = 0x002a_6b49;
const DEL_WORD: u32 = 0x0072_343a;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    Unstaged,
    Staged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Auto,
    Unified,
    Split,
}

/// The menu beside the diff.
#[derive(Debug, Clone, Copy)]
struct Options {
    wrap: bool,
    mode: Mode,
    /// The whole file around each change, not only a few lines of context.
    full_files: bool,
    /// A rendered view of a file that has one (markdown).
    rich_preview: bool,
    /// The words that changed, marked inside a changed line.
    word_diffs: bool,
    hide_whitespace: bool,
    hide_imports: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            wrap: false,
            mode: Mode::Auto,
            full_files: true,
            rich_preview: false,
            word_diffs: true,
            hide_whitespace: false,
            hide_imports: false,
        }
    }
}

/// A file's diff, read and prepared for drawing.
struct Loaded {
    /// What it was read for: a different key means it is read again.
    key: String,
    lines: Vec<Line>,
    words: HashMap<usize, Vec<Seg>>,
}

pub struct ChangesView {
    root: String,
    scope: Scope,
    options: Options,
    changes: Vec<FileChange>,
    expanded: HashSet<String>,
    diffs: HashMap<String, Loaded>,
    loading: HashSet<String>,
    read_once: bool,
}

impl ChangesView {
    pub fn new(root: String, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            root,
            scope: Scope::Unstaged,
            options: Options::default(),
            changes: Vec::new(),
            expanded: HashSet::new(),
            diffs: HashMap::new(),
            loading: HashSet::new(),
            read_once: false,
        };
        view.refresh(cx);
        // Kept current while it is on screen; the loop ends when the tab is closed.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                if this.update(cx, Self::refresh).is_err() {
                    return;
                }
            }
        })
        .detach();
        view
    }

    /// Reads the list of changed files; open diffs are read again only when their file changed.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let (root, staged, scope) = (self.root.clone(), self.scope == Scope::Staged, self.scope);
        let view = cx.entity().downgrade();
        crate::backend::run(
            cx,
            async move { vivianne_tools::git::diff_stat(&root, staged).await },
            move |list, cx| {
                view.update(cx, |view, cx| {
                    // The answer to a question asked before the scope changed is not an answer now.
                    if view.scope != scope {
                        return;
                    }
                    view.read_once = true;
                    // Only a change in the list is a change: a poll that finds the same files redraws nothing.
                    if view.changes != list {
                        view.changes = list;
                        view.load_open(cx);
                        cx.notify();
                    }
                })
                .ok();
            },
        );
    }

    fn whole(&self, path: &str) -> bool {
        self.options.full_files || (self.options.rich_preview && is_markdown(path))
    }

    fn key(&self, change: &FileChange) -> String {
        format!(
            "{:?}|{}|{}|{}|{}|{}",
            self.scope,
            change.path,
            change.added,
            change.removed,
            self.whole(&change.path),
            self.options.hide_whitespace
        )
    }

    /// Reads the diff of each open file that has not been read for its current state.
    fn load_open(&mut self, cx: &mut Context<Self>) {
        let wanted: Vec<FileChange> = self
            .changes
            .iter()
            .filter(|c| self.expanded.contains(&c.path))
            .cloned()
            .collect();
        for change in wanted {
            let key = self.key(&change);
            if self.diffs.get(&change.path).is_some_and(|d| d.key == key)
                || !self.loading.insert(change.path.clone())
            {
                continue;
            }
            let (root, staged, whitespace) = (
                self.root.clone(),
                self.scope == Scope::Staged,
                self.options.hide_whitespace,
            );
            let context = self.whole(&change.path).then_some(WHOLE_FILE);
            let (path, view) = (change.path.clone(), cx.entity().downgrade());
            crate::backend::run(
                cx,
                async move {
                    vivianne_tools::git::diff_file(&root, &path, staged, context, whitespace).await
                },
                move |raw, cx| {
                    view.update(cx, |view, cx| {
                        view.loading.remove(&change.path);
                        let mut lines = diff::parse(&raw);
                        if view.options.hide_imports {
                            lines = diff::hide_imports(&lines, &change.path);
                        }
                        let words = if view.options.word_diffs {
                            diff::word_diffs(&lines)
                        } else {
                            HashMap::new()
                        };
                        view.diffs
                            .insert(change.path.clone(), Loaded { key, lines, words });
                        cx.notify();
                    })
                    .ok();
                },
            );
        }
    }

    fn toggle(&mut self, path: &str, cx: &mut Context<Self>) {
        if !self.expanded.remove(path) {
            self.expanded.insert(path.to_owned());
            self.load_open(cx);
        }
        cx.notify();
    }

    fn toggle_all(&mut self, cx: &mut Context<Self>) {
        if self.expanded.len() == self.changes.len() {
            self.expanded.clear();
        } else {
            self.expanded = self.changes.iter().map(|c| c.path.clone()).collect();
            self.load_open(cx);
        }
        cx.notify();
    }

    fn set_scope(&mut self, scope: Scope, cx: &mut Context<Self>) {
        if self.scope == scope {
            return;
        }
        // Nothing from the other scope carries over.
        self.scope = scope;
        self.changes.clear();
        self.expanded.clear();
        self.diffs.clear();
        self.loading.clear();
        self.read_once = false;
        self.refresh(cx);
        cx.notify();
    }

    /// Changes how diffs are read or drawn; the ones that shape the text are read again.
    fn set_options(&mut self, options: Options, cx: &mut Context<Self>) {
        let rebuild = options.hide_imports != self.options.hide_imports
            || options.word_diffs != self.options.word_diffs;
        self.options = options;
        if rebuild {
            self.diffs.clear();
        }
        self.load_open(cx);
        cx.notify();
    }

    /// The `git apply` command for everything shown, on the clipboard.
    fn copy_apply(&self, cx: &mut Context<Self>) {
        let (root, staged, files) = (
            self.root.clone(),
            self.scope == Scope::Staged,
            self.changes
                .iter()
                .map(|c| c.path.clone())
                .collect::<Vec<_>>(),
        );
        crate::backend::run(
            cx,
            async move {
                let mut parts = Vec::new();
                for file in files {
                    parts.push(
                        vivianne_tools::git::diff_file(&root, &file, staged, None, false).await,
                    );
                }
                parts.retain(|p| !p.is_empty());
                diff::apply_command(&parts.join("\n"))
            },
            |command, cx| cx.write_to_clipboard(ClipboardItem::new_string(command)),
        );
    }
}

fn is_markdown(path: &str) -> bool {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    matches!(ext.as_str(), "md" | "markdown" | "mdx")
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn dir_name(path: &str) -> String {
    path.rsplit_once('/')
        .map_or(String::new(), |(dir, _)| format!("{dir}/"))
}

fn line_bg(kind: Kind) -> Option<u32> {
    match kind {
        Kind::Add => Some(ADD_BG),
        Kind::Del => Some(DEL_BG),
        Kind::Hunk => Some(theme::SURFACE),
        Kind::Ctx => None,
    }
}

fn gutter(n: Option<u32>) -> impl IntoElement {
    div()
        .w(px(44.0))
        .flex_none()
        .pr(px(8.0))
        .flex()
        .justify_end()
        .text_color(color(theme::FAINT))
        .child(n.map(|n| n.to_string()).unwrap_or_default())
}

/// A line's text, with the words that changed marked when they are known.
fn code(line: &Line, segs: Option<&Vec<Seg>>, wrap: bool) -> gpui_kit::AnyElement {
    let text = SharedString::from(line.text.clone());
    let mut highlights: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
    if let Some(segs) = segs {
        let mark = Hsla::from(rgb(if line.kind == Kind::Add {
            ADD_WORD
        } else {
            DEL_WORD
        }));
        let mut at = 0;
        for seg in segs {
            if seg.changed {
                highlights.push((
                    at..at + seg.text.len(),
                    HighlightStyle {
                        background_color: Some(mark),
                        ..HighlightStyle::default()
                    },
                ));
            }
            at += seg.text.len();
        }
    }
    let styled = StyledText::new(if text.is_empty() {
        SharedString::from(" ")
    } else {
        text
    })
    .with_highlights(highlights);
    let cell = div()
        .flex_1()
        .min_w_0()
        .px(px(8.0))
        .font_family(theme::MONO)
        .text_size(px(theme::TEXT_SM))
        .line_height(px(LINE_HEIGHT));
    if wrap {
        cell.child(styled).into_any_element()
    } else {
        cell.whitespace_nowrap().child(styled).into_any_element()
    }
}

fn unified(loaded: &Loaded, wrap: bool) -> gpui_kit::AnyElement {
    div()
        .flex()
        .flex_col()
        .children(
            loaded
                .lines
                .iter()
                .take(MAX_LINES)
                .enumerate()
                .map(|(i, l)| {
                    let sign = match l.kind {
                        Kind::Add => "+",
                        Kind::Del => "-",
                        _ => " ",
                    };
                    let mut row = div()
                        .w_full()
                        .min_h(px(LINE_HEIGHT))
                        .flex()
                        .text_color(color(if l.kind == Kind::Hunk {
                            theme::MUTED
                        } else {
                            theme::TEXT
                        }));
                    if let Some(bg) = line_bg(l.kind) {
                        row = row.bg(color(bg));
                    }
                    row.child(gutter(l.old_no))
                        .child(gutter(l.new_no))
                        .child(
                            div()
                                .flex_none()
                                .w(px(10.0))
                                .child(if l.kind == Kind::Hunk { "" } else { sign }),
                        )
                        .child(code(l, loaded.words.get(&i), wrap))
                }),
        )
        .into_any_element()
}

/// Side by side. Adds and deletes inside one run are zipped into rows, so a modified line sits opposite the one
/// it replaced.
fn split(loaded: &Loaded, wrap: bool) -> gpui_kit::AnyElement {
    let lines = &loaded.lines;
    let mut rows: Vec<(Option<usize>, Option<usize>)> = Vec::new();
    let mut i = 0;
    while i < lines.len() && rows.len() < MAX_LINES {
        match lines[i].kind {
            Kind::Ctx => {
                rows.push((Some(i), Some(i)));
                i += 1;
            }
            Kind::Hunk => {
                rows.push((Some(i), None));
                i += 1;
            }
            _ => {
                let (mut dels, mut adds) = (Vec::new(), Vec::new());
                while i < lines.len() && lines[i].kind == Kind::Del {
                    dels.push(i);
                    i += 1;
                }
                while i < lines.len() && lines[i].kind == Kind::Add {
                    adds.push(i);
                    i += 1;
                }
                for k in 0..dels.len().max(adds.len()) {
                    rows.push((dels.get(k).copied(), adds.get(k).copied()));
                }
            }
        }
    }
    let side = |index: Option<usize>, left: bool| {
        let mut cell = div().flex_1().min_w_0().flex().min_h(px(LINE_HEIGHT));
        match index.map(|i| (i, &lines[i])) {
            Some((i, l)) => {
                if let Some(bg) = line_bg(l.kind) {
                    cell = cell.bg(color(bg));
                }
                cell = cell.child(gutter(if left { l.old_no } else { l.new_no }));
                if l.kind != Kind::Hunk {
                    cell = cell.child(code(l, loaded.words.get(&i), wrap));
                }
            }
            None => cell = cell.bg(color(theme::BG)),
        }
        cell
    };
    div()
        .flex()
        .flex_col()
        .gap(px(1.0))
        .children(rows.into_iter().map(|(l, r)| {
            div()
                .w_full()
                .flex()
                .gap(px(1.0))
                .child(side(l, true))
                .child(side(r, false))
        }))
        .into_any_element()
}

fn rich_preview(path: &str, loaded: &Loaded) -> gpui_kit::AnyElement {
    let text: String = loaded
        .lines
        .iter()
        .filter(|l| matches!(l.kind, Kind::Ctx | Kind::Add))
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    div()
        .px(px(20.0))
        .py(px(16.0))
        .max_w(px(820.0))
        .child(
            TextView::markdown(SharedString::from(format!("review-{path}")), text).selectable(true),
        )
        .into_any_element()
}

impl Render for ChangesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let narrow = window.viewport_size().width < px(SPLIT_FROM_PX);
        let mode = match self.options.mode {
            Mode::Auto if narrow => Mode::Unified,
            Mode::Auto => Mode::Split,
            other => other,
        };
        let (added, removed) = self
            .changes
            .iter()
            .fold((0, 0), |(a, r), c| (a + c.added, r + c.removed));
        let all_open = !self.changes.is_empty() && self.expanded.len() == self.changes.len();
        let options = self.options;
        let me = cx.entity().downgrade();

        let scope_pill =
            |id: &'static str, label: &'static str, scope: Scope, cx: &mut Context<Self>| {
                super::ui::pill(
                    id,
                    format!(
                        "{label} · {}",
                        if self.scope == scope {
                            self.changes.len().to_string()
                        } else {
                            "–".to_owned()
                        }
                    ),
                    self.scope == scope,
                    cx.listener(move |view, _e, _w, cx| view.set_scope(scope, cx)),
                )
            };
        let mode_pill = |id: &'static str, label: &'static str, m: Mode, cx: &mut Context<Self>| {
            super::ui::pill(
                id,
                label,
                options.mode == m,
                cx.listener(move |view, _e, _w, cx| {
                    view.set_options(
                        Options {
                            mode: m,
                            ..view.options
                        },
                        cx,
                    )
                }),
            )
        };
        let toggle = |label: &'static str,
                      on: bool,
                      change: fn(&mut Options),
                      me: gpui_kit::WeakEntity<Self>| {
            PopupMenuItem::new(label)
                .checked(on)
                .on_click(move |_e, _w, app| {
                    me.update(app, |view, cx| {
                        let mut options = view.options;
                        change(&mut options);
                        view.set_options(options, cx);
                    })
                    .ok();
                })
        };

        let menu = Button::new("diff-options")
            .ghost()
            .xsmall()
            .label("···")
            .tooltip("View options")
            .dropdown_menu_with_anchor(Anchor::BottomRight, {
                let me = me.clone();
                move |menu, _window, _cx| {
                    let (refresh, all, copy) = (me.clone(), me.clone(), me.clone());
                    menu.item(PopupMenuItem::new("Refresh").on_click(move |_e, _w, app| {
                        refresh.update(app, Self::refresh).ok();
                    }))
                    .item(toggle(
                        "Word wrap",
                        options.wrap,
                        |o| o.wrap = !o.wrap,
                        me.clone(),
                    ))
                    .item(
                        PopupMenuItem::new(if all_open {
                            "Collapse all diffs"
                        } else {
                            "Expand all diffs"
                        })
                        .on_click(move |_e, _w, app| {
                            all.update(app, Self::toggle_all).ok();
                        }),
                    )
                    .separator()
                    .item(toggle(
                        "Load full files",
                        options.full_files,
                        |o| o.full_files = !o.full_files,
                        me.clone(),
                    ))
                    .item(toggle(
                        "Rich preview",
                        options.rich_preview,
                        |o| o.rich_preview = !o.rich_preview,
                        me.clone(),
                    ))
                    .item(toggle(
                        "Word diffs",
                        options.word_diffs,
                        |o| o.word_diffs = !o.word_diffs,
                        me.clone(),
                    ))
                    .item(toggle(
                        "Hide white space",
                        options.hide_whitespace,
                        |o| o.hide_whitespace = !o.hide_whitespace,
                        me.clone(),
                    ))
                    .item(toggle(
                        "Hide imports",
                        options.hide_imports,
                        |o| o.hide_imports = !o.hide_imports,
                        me.clone(),
                    ))
                    .item(
                        PopupMenuItem::new("Copy git apply command").on_click(
                            move |_e, _w, app| {
                                copy.update(app, |view, cx| view.copy_apply(cx)).ok();
                            },
                        ),
                    )
                }
            });

        let header = div()
            .h(px(36.0))
            .flex_none()
            .px(px(12.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .border_b_1()
            .border_color(color(theme::HAIRLINE))
            .child(scope_pill(
                "scope-unstaged",
                "Unstaged",
                Scope::Unstaged,
                cx,
            ))
            .child(scope_pill("scope-staged", "Staged", Scope::Staged, cx))
            .child(
                div()
                    .font_family(theme::MONO)
                    .text_size(px(theme::TEXT_SM))
                    .text_color(color(theme::OK))
                    .child(format!("+{added}")),
            )
            .child(
                div()
                    .font_family(theme::MONO)
                    .text_size(px(theme::TEXT_SM))
                    .text_color(color(theme::DANGER))
                    .child(format!("-{removed}")),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .gap(px(2.0))
                    .child(mode_pill("mode-auto", "Auto", Mode::Auto, cx))
                    .child(mode_pill("mode-unified", "Unified", Mode::Unified, cx))
                    .child(mode_pill("mode-split", "Split", Mode::Split, cx)),
            )
            .child(menu);

        let body: gpui_kit::AnyElement = if !self.read_once {
            div()
                .p(px(16.0))
                .child(Spinner::new().small())
                .into_any_element()
        } else if self.changes.is_empty() {
            div()
                .p(px(16.0))
                .child(muted(format!(
                    "No {} changes.",
                    if self.scope == Scope::Staged {
                        "staged"
                    } else {
                        "unstaged"
                    }
                )))
                .into_any_element()
        } else {
            {
                let rows: Vec<_> = self
                    .changes
                    .iter()
                    .enumerate()
                    .map(|(index, change)| self.file(index, change, mode, cx))
                    .collect();
                div().flex().flex_col().children(rows).into_any_element()
            }
        };
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(header)
            .child(
                div()
                    .id("changes-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(body),
            )
    }
}

impl ChangesView {
    fn file(
        &self,
        index: usize,
        change: &FileChange,
        mode: Mode,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let open = self.expanded.contains(&change.path);
        let path = change.path.clone();
        let head = div()
            .id(("change-file", index))
            .w_full()
            .h(px(28.0))
            .px(px(12.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .cursor_pointer()
            .hover(|style| style.bg(color(theme::SURFACE)))
            .on_click(cx.listener(move |view, _e, _w, cx| view.toggle(&path, cx)))
            .child(
                div()
                    .w(px(10.0))
                    .flex_none()
                    .text_color(color(theme::MUTED))
                    .child(if open { "▾" } else { "▸" }),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(theme::TEXT_SM))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(file_name(&change.path).to_owned()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(theme::TEXT_XS))
                    .text_color(color(theme::FAINT))
                    .child(dir_name(&change.path)),
            )
            .child(
                div()
                    .flex_none()
                    .font_family(theme::MONO)
                    .text_size(px(theme::TEXT_SM))
                    .text_color(color(theme::OK))
                    .child(format!("+{}", change.added)),
            )
            .child(
                div()
                    .flex_none()
                    .font_family(theme::MONO)
                    .text_size(px(theme::TEXT_SM))
                    .text_color(color(theme::DANGER))
                    .child(format!("-{}", change.removed)),
            );
        let body = open.then(|| {
            let shown = self
                .diffs
                .get(&change.path)
                .filter(|d| d.key == self.key(change))
                .or_else(|| self.diffs.get(&change.path));
            let inner = match shown {
                None => div()
                    .p(px(12.0))
                    .child(Spinner::new().small())
                    .into_any_element(),
                Some(d) if d.lines.is_empty() => div()
                    .p(px(12.0))
                    .child(muted(
                        if self.options.hide_imports || self.options.hide_whitespace {
                            "Nothing left to show with these options."
                        } else {
                            "No textual diff (binary file, or no changes)."
                        },
                    ))
                    .into_any_element(),
                Some(d) if self.options.rich_preview && is_markdown(&change.path) => {
                    rich_preview(&change.path, d)
                }
                Some(d) => {
                    let drawn = if mode == Mode::Split {
                        split(d, self.options.wrap)
                    } else {
                        unified(d, self.options.wrap)
                    };
                    let more = (d.lines.len() > MAX_LINES).then(|| {
                        div().p(px(8.0)).child(muted(format!(
                            "{} more lines not shown.",
                            d.lines.len() - MAX_LINES
                        )))
                    });
                    div()
                        .flex()
                        .flex_col()
                        .child(drawn)
                        .children(more)
                        .into_any_element()
                }
            };
            div()
                .id(("change-diff", index))
                .bg(color(theme::CODE_BG))
                .border_t_1()
                .border_color(color(theme::HAIRLINE))
                .overflow_x_scroll()
                .child(inner)
        });
        div()
            .border_b_1()
            .border_color(color(theme::HAIRLINE))
            .child(head)
            .children(body)
            .into_any_element()
    }
}
