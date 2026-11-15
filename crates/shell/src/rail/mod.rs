//! The project rail on the right of a chat: Overview, Files, Preview, Terminal.

mod files;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{Icon, IconName};
use gpui_kit::prelude::*;
use gpui_kit::{Context, Window, div, px};

use crate::theme::{self, color};

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
    body: Result<String, String>,
}

pub struct Rail {
    root: PathBuf,
    tab: RailTab,
    expanded: HashSet<PathBuf>,
    preview: Option<Preview>,
    branch: Option<String>,
}

impl Rail {
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        let branch = git_branch(&root);
        Self {
            root,
            tab: RailTab::Overview,
            expanded: HashSet::new(),
            preview: None,
            branch,
        }
    }

    fn open_row(&mut self, path: PathBuf, is_dir: bool) {
        if is_dir {
            if !self.expanded.remove(&path) {
                self.expanded.insert(path);
            }
            return;
        }
        let body = files::read_preview(&path);
        self.preview = Some(Preview { path, body });
        self.tab = RailTab::Preview;
    }

    fn tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = TABS
            .iter()
            .position(|(tab, _)| *tab == self.tab)
            .unwrap_or(0);
        TabBar::new("rail-tabs")
            .underline()
            .selected_index(selected)
            .on_click(cx.listener(|rail, index: &usize, _window, cx| {
                if let Some((tab, _)) = TABS.get(*index) {
                    rail.tab = *tab;
                    cx.notify();
                }
            }))
            .children(TABS.iter().map(|(_, label)| Tab::new().label(*label)))
    }

    fn overview(&self) -> impl IntoElement {
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
                    div().child(folder).into_any_element(),
                    muted(self.root.display().to_string()).into_any_element(),
                ],
            ))
            .child(section(
                "Version control",
                [match &self.branch {
                    Some(branch) => div().child(format!("Branch: {branch}")).into_any_element(),
                    None => muted("Not a git repository").into_any_element(),
                }],
            ))
            .child(section(
                "Context",
                [muted("Usage appears once the agent has run.").into_any_element()],
            ))
    }

    fn files(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = files::visible_rows(&self.root, &self.expanded);
        div()
            .flex()
            .flex_col()
            .children(rows.into_iter().enumerate().map(|(index, row)| {
                let icon = match (row.is_dir, row.expanded) {
                    (true, true) => IconName::FolderOpen,
                    (true, false) => IconName::Folder,
                    (false, _) => IconName::File,
                };
                let path = row.path.clone();
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
                    .on_click(cx.listener(move |rail, _event, _window, cx| {
                        rail.open_row(path.clone(), row.is_dir);
                        cx.notify();
                    }))
                    .child(Icon::new(icon).size_4().text_color(color(theme::MUTED)))
                    .child(div().child(row.name))
            }))
    }

    fn preview(&self) -> impl IntoElement {
        let Some(preview) = &self.preview else {
            return muted("Open a file from Files to preview it here.").into_any_element();
        };
        let name = preview
            .path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let body = match &preview.body {
            Ok(text) => text
                .lines()
                .take(PREVIEW_MAX_LINES)
                .collect::<Vec<_>>()
                .join("\n"),
            Err(why) => why.clone(),
        };
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(div().child(name))
            .child(
                div()
                    .id("preview-body")
                    .max_h(px(520.0))
                    .overflow_y_scroll()
                    .p(px(10.0))
                    .rounded(px(theme::RADIUS_SURFACE))
                    .border_1()
                    .border_color(color(theme::BORDER))
                    .bg(color(theme::CODE_BG))
                    .font_family(theme::MONO)
                    .text_size(px(12.0))
                    .child(body),
            )
            .into_any_element()
    }
}

impl Render for Rail {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match self.tab {
            RailTab::Overview => self.overview().into_any_element(),
            RailTab::Files => self.files(cx).into_any_element(),
            RailTab::Preview => self.preview().into_any_element(),
            RailTab::Terminal => muted("The terminal view is not built yet.").into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .rounded(px(theme::RADIUS_SURFACE))
            .border_1()
            .border_color(color(theme::BORDER))
            .bg(color(theme::BG))
            .child(div().px(px(8.0)).child(self.tab_bar(cx)))
            .child(
                div()
                    .id("rail-content")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(px(12.0))
                    .text_size(px(13.0))
                    .child(content),
            )
    }
}

fn section<const N: usize>(title: &str, lines: [gpui_kit::AnyElement; N]) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(
            div()
                .text_size(px(11.0))
                .text_color(color(theme::MUTED))
                .child(title.to_uppercase()),
        )
        .children(lines)
}

fn muted(text: impl Into<gpui_kit::SharedString>) -> gpui_kit::Div {
    div().text_color(color(theme::MUTED)).child(text.into())
}

/// Current branch of the git repository at `dir`, if it is one.
fn git_branch(dir: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(dir)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
        .filter(|branch| !branch.is_empty())
}
