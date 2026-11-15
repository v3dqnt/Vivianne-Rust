//! One tool call in the transcript: a header (what ran, on what, how it went)
//! and, below it, a view made for that tool's output. Click the header to fold it.

mod bodies;
mod parse;

use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Icon, IconName, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, Hsla, SharedString, div, px, rgb};

use super::ChatView;
use super::items::ToolItem;
use crate::theme::{self, color};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tone {
    Plain,
    Good,
    Bad,
}

struct Header {
    icon: IconName,
    verb: &'static str,
    /// What it ran on: a path, a pattern, a command.
    target: String,
    chip: Option<(String, Tone)>,
}

pub fn render(tool: &ToolItem, folded: bool, cx: &mut Context<ChatView>) -> AnyElement {
    let header = header_of(tool);
    let running = tool.output.is_none();
    let id = tool.id.clone();
    let row = div()
        .id(SharedString::from(format!("tool-{}", tool.id)))
        .flex()
        .items_center()
        .gap(px(8.0))
        .py(px(2.0))
        .cursor_pointer()
        .text_size(px(13.0))
        .on_click(cx.listener(move |chat, _event, _window, cx| chat.toggle_tool(&id, cx)))
        .child(icon_slot(&header, running))
        .child(div().text_color(color(theme::MUTED)).child(header.verb))
        .child(
            div()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .font_family(theme::MONO)
                .text_size(px(12.0))
                .child(header.target),
        )
        .children(chip(header.chip.as_ref(), running))
        .child(
            Icon::new(if folded {
                IconName::ChevronRight
            } else {
                IconName::ChevronDown
            })
            .size_3()
            .text_color(color(theme::MUTED)),
        );
    let body = (!folded && !running)
        .then(|| bodies::render(tool))
        .flatten();
    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(row)
        .when_some(body, |column, body| {
            column.child(div().pl(px(22.0)).child(body))
        })
        .into_any_element()
}

fn icon_slot(header: &Header, running: bool) -> AnyElement {
    if running {
        return Spinner::new()
            .small()
            .color(Hsla::from(rgb(theme::ACCENT)))
            .into_any_element();
    }
    Icon::new(header.icon.clone())
        .size_4()
        .text_color(color(theme::MUTED))
        .into_any_element()
}

fn chip(chip: Option<&(String, Tone)>, running: bool) -> Option<AnyElement> {
    let (text, tone) = match (chip, running) {
        (_, true) => ("Running…".to_owned(), Tone::Plain),
        (Some((text, tone)), false) => (text.clone(), *tone),
        (None, false) => return None,
    };
    let text_color = match tone {
        Tone::Plain => theme::MUTED,
        Tone::Good => theme::OK,
        Tone::Bad => theme::DANGER,
    };
    Some(
        div()
            .px(px(6.0))
            .py(px(1.0))
            .rounded(px(theme::RADIUS_CONTROL))
            .border_1()
            .border_color(color(theme::BORDER))
            .text_size(px(11.0))
            .text_color(color(text_color))
            .child(text)
            .into_any_element(),
    )
}

fn arg<'a>(tool: &'a ToolItem, key: &str) -> Option<&'a str> {
    tool.args[key].as_str().filter(|s| !s.is_empty())
}

fn detail_u64(tool: &ToolItem, key: &str) -> Option<u64> {
    tool.details.as_ref()?[key].as_u64()
}

fn detail_str<'a>(tool: &'a ToolItem, key: &str) -> Option<&'a str> {
    tool.details.as_ref()?[key]
        .as_str()
        .filter(|s| !s.is_empty())
}

fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// A flat match: one arm per tool, so it stays one long function.
#[allow(clippy::too_many_lines)]
fn header_of(tool: &ToolItem) -> Header {
    let path = arg(tool, "path").unwrap_or(".").to_owned();
    let plain = |chip: Option<String>| chip.map(|c| (c, Tone::Plain));
    let mut header = match tool.name.as_str() {
        "read" => {
            let (lines, shown) = (detail_u64(tool, "lines"), detail_u64(tool, "shown"));
            let chip = match (lines, shown) {
                (Some(l), Some(s)) if s < l => Some(format!("{s} of {l} lines")),
                (Some(l), _) => Some(plural(l, "line", "lines")),
                _ => None,
            };
            Header {
                icon: IconName::FileText,
                verb: "Read",
                target: path,
                chip: plain(chip),
            }
        }
        "ls" => Header {
            icon: IconName::FolderOpen,
            verb: "Listed",
            target: path,
            chip: plain(detail_u64(tool, "entries").map(|n| plural(n, "entry", "entries"))),
        },
        "grep" => {
            let partial = tool
                .details
                .as_ref()
                .is_some_and(|d| d["scan_capped"] == true);
            let chip = detail_u64(tool, "matches").map(|n| {
                format!(
                    "{}{}",
                    plural(n, "match", "matches"),
                    if partial { ", partial" } else { "" }
                )
            });
            let pattern = arg(tool, "pattern").unwrap_or_default();
            Header {
                icon: IconName::Search,
                verb: "Searched",
                target: format!("/{pattern}/"),
                chip: plain(chip),
            }
        }
        "glob" => {
            let count = tool
                .output
                .as_deref()
                .and_then(parse::glob)
                .map(|g| g.paths.len() as u64);
            Header {
                icon: IconName::Search,
                verb: "Found files",
                target: arg(tool, "pattern").unwrap_or_default().to_owned(),
                chip: plain(count.map(|n| plural(n, "file", "files"))),
            }
        }
        "code_search" => {
            let mode = if tool.details.as_ref().is_some_and(|d| d["semantic"] == true) {
                "semantic"
            } else {
                "keyword"
            };
            Header {
                icon: IconName::Search,
                verb: "Searched code",
                target: arg(tool, "query").unwrap_or_default().to_owned(),
                chip: plain(
                    detail_u64(tool, "results")
                        .map(|n| format!("{}, {mode}", plural(n, "result", "results"))),
                ),
            }
        }
        "repo_clone" => Header {
            icon: IconName::Github,
            verb: "Repository",
            target: detail_str(tool, "repository")
                .or(arg(tool, "repository"))
                .unwrap_or_default()
                .to_owned(),
            chip: plain(detail_str(tool, "status").map(str::to_owned)),
        },
        "repo_overview" => Header {
            icon: IconName::LayoutDashboard,
            verb: "Overview",
            target: detail_str(tool, "repository").map_or(path, str::to_owned),
            chip: plain(detail_str(tool, "packageManager").map(str::to_owned)),
        },
        "diagnostics" => {
            let chip = tool.output.as_deref().map(|o| match parse::diagnostics(o) {
                parse::Problems::Clean(_) => ("No problems".to_owned(), Tone::Good),
                parse::Problems::Unchecked(_) => ("Not checked".to_owned(), Tone::Plain),
                parse::Problems::Found(lines) => {
                    (plural(lines.len() as u64, "line", "lines"), Tone::Bad)
                }
            });
            Header {
                icon: IconName::TriangleAlert,
                verb: "Checked",
                target: path,
                chip,
            }
        }
        "lsp" => {
            let operation = arg(tool, "operation").unwrap_or("query");
            let at = arg(tool, "path").map(|p| {
                format!(
                    " {p}{}",
                    tool.args["line"]
                        .as_u64()
                        .map_or(String::new(), |l| format!(":{l}"))
                )
            });
            Header {
                icon: IconName::Network,
                verb: "Language server",
                target: format!("{operation}{}", at.unwrap_or_default()),
                chip: None,
            }
        }
        "shell" => {
            let code = tool.details.as_ref().and_then(|d| d["exit_code"].as_i64());
            let chip = code.map(|c| {
                (
                    format!("exit {c}"),
                    if c == 0 { Tone::Good } else { Tone::Bad },
                )
            });
            Header {
                icon: IconName::SquareTerminal,
                verb: "Ran",
                target: arg(tool, "command").unwrap_or_default().to_owned(),
                chip,
            }
        }
        "write" => Header {
            icon: IconName::File,
            verb: "Wrote",
            target: path,
            chip: plain(detail_u64(tool, "bytes").map(|n| format!("{n} bytes"))),
        },
        "edit" => Header {
            icon: IconName::Replace,
            verb: "Edited",
            target: path,
            chip: None,
        },
        other => Header {
            icon: IconName::Bot,
            verb: "Used",
            target: other.to_owned(),
            chip: None,
        },
    };
    if tool.is_error {
        header.chip = Some(("Failed".to_owned(), Tone::Bad));
    }
    header
}
