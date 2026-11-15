//! The view under a tool call's header, one per tool. Each falls back to the
//! plain output when the text is not in the shape the tool normally writes.

use gpui_kit::component::{Icon, IconName};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, SharedString, div, px};

use super::parse::{self, Problems};
use super::{arg, detail_str};
use crate::chat::items::ToolItem;
use crate::theme::{self, color};

/// Tallest a body gets before it scrolls.
const MAX_HEIGHT: f32 = 260.0;
const GUTTER: f32 = 44.0;

pub fn render(tool: &ToolItem) -> Option<AnyElement> {
    let output = tool.output.as_deref()?;
    if tool.is_error {
        return Some(plain(tool, output, true));
    }
    let view = match tool.name.as_str() {
        "read" => parse::read(output).map(|v| read_view(tool, &v)),
        "ls" => Some(ls_view(tool, &parse::ls(output))),
        "grep" => parse::grep(output).map(|v| grep_view(tool, &v)),
        "glob" => parse::glob(output).map(|v| glob_view(tool, &v)),
        "code_search" => parse::code_search(output).map(|v| search_view(tool, &v)),
        "repo_clone" => Some(facts_view(tool, &parse::key_values(output))),
        "repo_overview" => Some(overview_view(tool, output)),
        "diagnostics" => Some(problems_view(tool, &parse::diagnostics(output))),
        "edit" => Some(diff_view(tool)),
        _ => None,
    };
    view.or_else(|| Some(plain(tool, output, false)))
}

/// The scrollable, bordered box all bodies sit in.
fn panel(tool: &ToolItem, kind: &str) -> gpui_kit::Stateful<gpui_kit::Div> {
    div()
        .id(SharedString::from(format!("body-{kind}-{}", tool.id)))
        .max_h(px(MAX_HEIGHT))
        .overflow_scroll()
        .p(px(10.0))
        .rounded(px(theme::RADIUS_SURFACE))
        .border_1()
        .border_color(color(theme::BORDER))
        .bg(color(theme::CODE_BG))
        .text_size(px(12.0))
}

fn mono(text: impl Into<SharedString>) -> gpui_kit::Div {
    div()
        .font_family(theme::MONO)
        .whitespace_nowrap()
        .child(text.into())
}

fn muted(text: impl Into<SharedString>) -> gpui_kit::Div {
    div().text_color(color(theme::MUTED)).child(text.into())
}

fn note(text: &str) -> AnyElement {
    muted(text.to_owned()).pt(px(6.0)).into_any_element()
}

fn plain(tool: &ToolItem, output: &str, is_error: bool) -> AnyElement {
    let text = if is_error { theme::DANGER } else { theme::TEXT };
    panel(tool, "plain")
        .text_color(color(text))
        .child(mono(output.to_owned()))
        .into_any_element()
}

fn read_view(tool: &ToolItem, view: &parse::ReadView) -> AnyElement {
    panel(tool, "read")
        .flex()
        .flex_col()
        .children(view.rows.iter().map(|(number, text)| {
            div()
                .flex()
                .gap(px(12.0))
                .child(
                    div()
                        .w(px(GUTTER))
                        .flex_none()
                        .text_color(color(theme::MUTED))
                        .font_family(theme::MONO)
                        .child(number.to_string()),
                )
                .child(mono(if text.is_empty() {
                    " ".to_owned()
                } else {
                    text.clone()
                }))
        }))
        .children(view.notes.iter().map(|n| note(n)))
        .into_any_element()
}

fn ls_view(tool: &ToolItem, view: &parse::LsView) -> AnyElement {
    panel(tool, "ls")
        .flex()
        .flex_col()
        .gap(px(2.0))
        .children(view.rows.iter().map(|row| {
            let icon = if row.is_dir {
                IconName::Folder
            } else {
                IconName::File
            };
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .pl(px(
                    f32::from(u8::try_from(row.depth).unwrap_or(u8::MAX)) * 16.0
                ))
                .child(Icon::new(icon).size_3p5().text_color(color(theme::MUTED)))
                .child(div().child(row.name.clone()))
        }))
        .children(view.notes.iter().map(|n| note(n)))
        .into_any_element()
}

fn grep_view(tool: &ToolItem, view: &parse::GrepView) -> AnyElement {
    panel(tool, "grep")
        .flex()
        .flex_col()
        .gap(px(10.0))
        .children(view.files.iter().map(|(path, hits)| {
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .flex()
                        .gap(px(8.0))
                        .child(mono(path.clone()))
                        .child(muted(format!("{}", hits.len()))),
                )
                .children(hits.iter().map(|(line, text)| {
                    div()
                        .flex()
                        .gap(px(12.0))
                        .child(
                            div()
                                .w(px(GUTTER))
                                .flex_none()
                                .text_color(color(theme::MUTED))
                                .font_family(theme::MONO)
                                .child(line.to_string()),
                        )
                        .child(mono(text.trim_end().to_owned()))
                }))
        }))
        .children(view.notes.iter().map(|n| note(n)))
        .into_any_element()
}

fn glob_view(tool: &ToolItem, view: &parse::GlobView) -> AnyElement {
    if view.paths.is_empty() {
        return panel(tool, "glob")
            .child(muted("No files found"))
            .into_any_element();
    }
    panel(tool, "glob")
        .flex()
        .flex_col()
        .gap(px(2.0))
        .children(view.paths.iter().map(|path| {
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    Icon::new(IconName::File)
                        .size_3p5()
                        .text_color(color(theme::MUTED)),
                )
                .child(mono(path.clone()))
        }))
        .children(view.note.iter().map(|n| note(n)))
        .into_any_element()
}

fn search_view(tool: &ToolItem, view: &parse::SearchView) -> AnyElement {
    panel(tool, "search")
        .flex()
        .flex_col()
        .gap(px(12.0))
        .children(view.hits.iter().map(|hit| {
            div()
                .flex()
                .flex_col()
                .gap(px(3.0))
                .child(
                    div()
                        .flex()
                        .gap(px(8.0))
                        .child(mono(hit.location.clone()))
                        .children(hit.score.iter().map(|s| muted(format!("score {s}")))),
                )
                .children(
                    hit.preview
                        .iter()
                        .map(|line| mono(line.clone()).text_color(color(theme::MUTED))),
                )
        }))
        .children(view.notes.iter().map(|n| note(n)))
        .into_any_element()
}

/// Label and value rows, for `repo_clone`.
fn facts_view(tool: &ToolItem, facts: &[(String, String)]) -> AnyElement {
    let rows = facts
        .iter()
        .filter(|(key, _)| !key.starts_with("Repository"));
    panel(tool, "facts")
        .flex()
        .flex_col()
        .gap(px(4.0))
        .children(rows.map(|(key, value)| fact_row(key, value.clone())))
        .into_any_element()
}

fn fact_row(label: &str, value: String) -> gpui_kit::Div {
    div()
        .flex()
        .gap(px(12.0))
        .child(
            div()
                .w(px(110.0))
                .flex_none()
                .text_color(color(theme::MUTED))
                .child(label.to_owned()),
        )
        .child(mono(value))
}

/// `repo_overview`: what it found, then the structure it printed.
fn overview_view(tool: &ToolItem, output: &str) -> AnyElement {
    let details = tool.details.as_ref();
    let list = |key: &str| -> Vec<String> {
        details
            .and_then(|d| d[key].as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut facts: Vec<(&str, String)> = Vec::new();
    for (label, key) in [
        ("Ecosystems", "ecosystems"),
        ("Dependencies", "dependencyFiles"),
        ("Entrypoints", "entrypoints"),
    ] {
        let values = list(key);
        if !values.is_empty() {
            facts.push((label, values.join(", ")));
        }
    }
    if let Some(pm) = detail_str(tool, "packageManager") {
        facts.insert(0, ("Package manager", pm.to_owned()));
    }
    let structure: Vec<&str> = output
        .lines()
        .skip_while(|l| !l.starts_with("Top-level structure"))
        .skip(1)
        .collect();
    panel(tool, "overview")
        .flex()
        .flex_col()
        .gap(px(4.0))
        .children(
            facts
                .into_iter()
                .map(|(label, value)| fact_row(label, value)),
        )
        .when(!structure.is_empty(), |panel| {
            panel
                .child(div().h(px(6.0)))
                .children(structure.into_iter().map(|line| mono(line.to_owned())))
        })
        .into_any_element()
}

fn problems_view(tool: &ToolItem, problems: &Problems) -> AnyElement {
    match problems {
        Problems::Clean(text) => panel(tool, "ok")
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(
                Icon::new(IconName::CircleCheck)
                    .size_4()
                    .text_color(color(theme::OK)),
            )
            .child(div().child(text.clone()))
            .into_any_element(),
        Problems::Unchecked(text) => panel(tool, "unchecked")
            .child(muted(text.clone()))
            .into_any_element(),
        Problems::Found(lines) => panel(tool, "problems")
            .flex()
            .flex_col()
            .gap(px(3.0))
            .children(lines.iter().map(|line| {
                let tint = if line.contains("ERROR") || line.starts_with("error") {
                    theme::DANGER
                } else {
                    theme::TEXT
                };
                mono(line.clone()).text_color(color(tint))
            }))
            .into_any_element(),
    }
}

/// An edit as removed and added lines.
fn diff_view(tool: &ToolItem) -> AnyElement {
    let side = |key: &str, mark: &'static str, tint: u32| {
        tool.args[key]
            .as_str()
            .unwrap_or_default()
            .lines()
            .map(|line| mono(format!("{mark} {line}")).text_color(color(tint)))
            .collect::<Vec<_>>()
    };
    let _ = arg(tool, "path");
    panel(tool, "diff")
        .flex()
        .flex_col()
        .children(side("old_string", "-", theme::DANGER))
        .children(side("new_string", "+", theme::OK))
        .into_any_element()
}
