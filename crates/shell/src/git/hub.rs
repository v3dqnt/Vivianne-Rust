//! The GitHub tabs: pull requests with their checks and reviews, Actions runs with their jobs and logs, issues,
//! and releases. Each is a list on the left and the picked item on the right, read from GitHub as the user opens
//! it; the buttons act as the user, so they are not gated.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use gpui_kit::base::input::TextareaState;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Textarea;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::text::TextView;
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, Entity, EventEmitter, FontWeight, SharedString, Subscription, Window, div,
    px,
};
use serde_json::{Value, json};
use vivianne_tools::git::time;

use super::ui::{
    self, Act, Load, at, block, detail_head, list, list_row, muted, n, pill, pr_mark, problem, s,
    split, status_mark,
};
use crate::theme::{self, color};

/// What a GitHub tab asks the page to do.
#[derive(Debug, Clone)]
pub enum HubEvent {
    /// Check out a pull request's branch.
    Checkout { branch: String, pr: u64 },
    /// Something changed on GitHub: the header's counts are stale.
    Changed,
}

fn markdown(id: impl Into<SharedString>, text: &str) -> AnyElement {
    div()
        .text_size(px(13.0))
        .child(TextView::markdown(id.into(), text.to_owned()).selectable(true))
        .into_any_element()
}

fn loading() -> AnyElement {
    div()
        .p(px(16.0))
        .child(Spinner::new().small())
        .into_any_element()
}

fn textarea<V: 'static>(
    rows: usize,
    placeholder: &'static str,
    window: &mut Window,
    cx: &mut Context<V>,
) -> Entity<TextareaState> {
    cx.new(|cx| {
        TextareaState::new(window, cx)
            .auto_grow(rows, 8)
            .placeholder(placeholder)
    })
}

fn bordered(child: impl IntoElement) -> Div {
    div()
        .rounded(px(theme::RADIUS_CONTROL))
        .border_1()
        .border_color(color(theme::BORDER))
        .px(px(8.0))
        .py(px(4.0))
        .child(child)
}
use gpui_kit::Div;

// ── Pull requests ─────────────────────────────────────────────────────────────────────────────────────────────

struct PrDetail {
    number: i64,
    pr: Load<Value>,
    files: Load<Vec<Value>>,
    checks: Load<Vec<Value>>,
    reviews: Load<Vec<Value>>,
}

pub struct Prs {
    root: String,
    closed: bool,
    list: Load<Vec<Value>>,
    picked: Option<PrDetail>,
    act: Act,
    comment: Entity<TextareaState>,
    open_files: HashSet<String>,
}

impl EventEmitter<HubEvent> for Prs {}

impl Prs {
    pub fn new(root: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let comment = textarea(2, "Leave a comment", window, cx);
        let mut this = Self {
            root,
            closed: false,
            list: Load::Loading,
            picked: None,
            act: Act::default(),
            comment,
            open_files: HashSet::new(),
        };
        this.reload(cx);
        this
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let state = if self.closed { "closed" } else { "open" };
        let want = self.closed;
        ui::call(
            cx,
            &self.root.clone(),
            format!("/repos/{{repo}}/pulls?state={state}&per_page=50&sort=updated&direction=desc"),
            "GET",
            None,
            move |this, answer, cx| {
                if this.closed == want {
                    this.list = Load::from(answer.map(|v| list(&v).to_vec()));
                    cx.notify();
                }
            },
        );
    }

    fn open(&mut self, number: i64, cx: &mut Context<Self>) {
        self.picked = Some(PrDetail {
            number,
            pr: Load::Loading,
            files: Load::Loading,
            checks: Load::Loading,
            reviews: Load::Loading,
        });
        self.open_files.clear();
        self.act = Act::default();
        let root = self.root.clone();
        ui::call(
            cx,
            &root,
            format!("/repos/{{repo}}/pulls/{number}"),
            "GET",
            None,
            move |this, answer, cx| {
                let Some(d) = this.picked.as_mut().filter(|d| d.number == number) else {
                    return;
                };
                let sha = answer
                    .as_ref()
                    .ok()
                    .map(|p| at(p, &["head", "sha"]).to_owned());
                d.pr = Load::from(answer);
                cx.notify();
                if let Some(sha) = sha {
                    let root = this.root.clone();
                    ui::call(
                        cx,
                        &root,
                        format!("/repos/{{repo}}/commits/{sha}/check-runs?per_page=100"),
                        "GET",
                        None,
                        move |this, answer, cx| {
                            if let Some(d) = this.picked.as_mut().filter(|d| d.number == number) {
                                d.checks = Load::from(answer.map(|v| {
                                    list(v.get("check_runs").unwrap_or(&Value::Null)).to_vec()
                                }));
                                cx.notify();
                            }
                        },
                    );
                }
            },
        );
        ui::call(
            cx,
            &root,
            format!("/repos/{{repo}}/pulls/{number}/files?per_page=100"),
            "GET",
            None,
            move |this, answer, cx| {
                if let Some(d) = this.picked.as_mut().filter(|d| d.number == number) {
                    d.files = Load::from(answer.map(|v| list(&v).to_vec()));
                    cx.notify();
                }
            },
        );
        ui::call(
            cx,
            &root,
            format!("/repos/{{repo}}/pulls/{number}/reviews?per_page=50"),
            "GET",
            None,
            move |this, answer, cx| {
                if let Some(d) = this.picked.as_mut().filter(|d| d.number == number) {
                    d.reviews = Load::from(answer.map(|v| list(&v).to_vec()));
                    cx.notify();
                }
            },
        );
        cx.notify();
    }

    /// A change to the picked pull request: sent to GitHub, then everything read again.
    fn change(
        &mut self,
        label: &'static str,
        confirm: bool,
        path: String,
        method: &'static str,
        body: Value,
        cx: &mut Context<Self>,
    ) {
        let Some(number) = self.picked.as_ref().map(|d| d.number) else {
            return;
        };
        if !self.act.click(label, confirm) {
            cx.notify();
            return;
        }
        let root = self.root.clone();
        ui::call(
            cx,
            &root,
            path,
            method,
            Some(body),
            move |this, answer, cx| {
                this.act.done(&answer);
                if answer.is_ok() {
                    this.open(number, cx);
                    this.reload(cx);
                    cx.emit(HubEvent::Changed);
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    fn detail(&self, d: &PrDetail, cx: &mut Context<Self>) -> AnyElement {
        if let Some(e) = d.pr.error() {
            return problem(e).into_any_element();
        }
        let Some(p) = d.pr.data() else {
            return loading();
        };
        let number = d.number;
        let merged = !s(p, "merged_at").is_empty();
        let open = s(p, "state") == "open" && !merged;
        let state = if merged {
            "Merged"
        } else if s(p, "state") == "closed" {
            "Closed"
        } else if p.get("draft").and_then(Value::as_bool).unwrap_or(false) {
            "Draft"
        } else {
            "Open"
        };
        let (base, head) = (
            at(p, &["base", "ref"]).to_owned(),
            at(p, &["head", "ref"]).to_owned(),
        );
        let commits = n(p, "commits");
        let sub = div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .flex_wrap()
            .child(pr_mark(p))
            .child(format!(
                "{state} · {} wants to merge {commits} commit{} from",
                at(p, &["user", "login"]),
                if commits == 1 { "" } else { "s" }
            ))
            .child(ui::mono(head.clone()))
            .child("into")
            .child(ui::mono(base.clone()))
            .child(
                div()
                    .text_color(color(theme::OK))
                    .child(format!("+{}", n(p, "additions"))),
            )
            .child(
                div()
                    .text_color(color(theme::DANGER))
                    .child(format!("−{}", n(p, "deletions"))),
            );
        let mut actions = vec![{
            let (branch, pr) = (head.clone(), u64::try_from(number).unwrap_or(0));
            Button::new("pr-checkout")
                .outline()
                .xsmall()
                .label("Check out")
                .on_click(cx.listener(move |_this, _e, _w, cx| {
                    cx.emit(HubEvent::Checkout {
                        branch: branch.clone(),
                        pr,
                    })
                }))
                .into_any_element()
        }];
        let put = format!("/repos/{{repo}}/pulls/{number}");
        if open {
            let path = format!("{put}/merge");
            actions.push(self.act.button(
                "pr-merge",
                "Squash and merge",
                false,
                cx.listener(move |this, _e, _w, cx| {
                    this.change(
                        "Squash and merge",
                        true,
                        path.clone(),
                        "PUT",
                        json!({ "merge_method": "squash" }),
                        cx,
                    )
                }),
            ));
            let path = put.clone();
            actions.push(self.act.button(
                "pr-close",
                "Close",
                true,
                cx.listener(move |this, _e, _w, cx| {
                    this.change(
                        "Close",
                        false,
                        path.clone(),
                        "PATCH",
                        json!({ "state": "closed" }),
                        cx,
                    )
                }),
            ));
        } else if !merged {
            let path = put.clone();
            actions.push(self.act.button(
                "pr-reopen",
                "Reopen",
                false,
                cx.listener(move |this, _e, _w, cx| {
                    this.change(
                        "Reopen",
                        false,
                        path.clone(),
                        "PATCH",
                        json!({ "state": "open" }),
                        cx,
                    )
                }),
            ));
        }
        let now = time::now();

        let checks = block(
            format!(
                "Checks{}",
                d.checks
                    .data()
                    .map_or(String::new(), |c| format!(" · {}", c.len()))
            ),
            match d.checks.data() {
                Some(c) if c.is_empty() => {
                    vec![muted("No checks on this commit.").into_any_element()]
                }
                Some(c) => c
                    .iter()
                    .map(|c| {
                        let url = s(c, "html_url").to_owned();
                        div()
                            .h(px(24.0))
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(status_mark(s(c, "status"), s(c, "conclusion")))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(theme::TEXT_SM))
                                    .child(s(c, "name").to_owned()),
                            )
                            .child(muted(time::duration(
                                c.get("started_at").and_then(Value::as_str),
                                c.get("completed_at").and_then(Value::as_str),
                                now,
                            )))
                            .children((!url.is_empty()).then(|| {
                                Button::new(SharedString::from(format!("check-{}", n(c, "id"))))
                                    .ghost()
                                    .xsmall()
                                    .label("Details")
                                    .on_click(move |_e, _w, cx| cx.open_url(&url))
                            }))
                            .into_any_element()
                    })
                    .collect(),
                None => vec![loading()],
            },
        );
        let reviews = d.reviews.data().filter(|r| !r.is_empty()).map(|r| {
            block(
                "Reviews",
                r.iter()
                    .map(|r| {
                        let verb = match s(r, "state") {
                            "APPROVED" => "approved",
                            "CHANGES_REQUESTED" => "requested changes",
                            _ => "commented",
                        };
                        muted(format!(
                            "{} {verb} · {}",
                            at(r, &["user", "login"]),
                            time::ago(s(r, "submitted_at"), now)
                        ))
                        .into_any_element()
                    })
                    .collect(),
            )
        });
        let description = (!s(p, "body").is_empty()).then(|| {
            block(
                "Description",
                vec![markdown(format!("pr-{number}-body"), s(p, "body"))],
            )
        });
        let files = block(
            format!(
                "Files changed{}",
                d.files
                    .data()
                    .map_or(String::new(), |f| format!(" · {}", f.len()))
            ),
            match d.files.data() {
                Some(f) => f
                    .iter()
                    .enumerate()
                    .map(|(i, f)| self.file(i, f, cx))
                    .collect(),
                None => vec![loading()],
            },
        );
        let comment = block(
            "Comment",
            vec![
                bordered(Textarea::new(&self.comment).appearance(false)).into_any_element(),
                div()
                    .pt(px(6.0))
                    .child(self.act.button(
                        "pr-comment",
                        "Comment",
                        false,
                        cx.listener(move |this, _e, window, cx| {
                            let text = this.comment.read(cx).value().trim().to_owned();
                            if text.is_empty() {
                                return;
                            }
                            this.comment.update(cx, |c, cx| c.set_value("", window, cx));
                            this.change(
                                "Comment",
                                false,
                                format!("/repos/{{repo}}/issues/{number}/comments"),
                                "POST",
                                json!({ "body": text }),
                                cx,
                            );
                        }),
                    ))
                    .into_any_element(),
            ],
        );
        div()
            .flex()
            .flex_col()
            .child(detail_head(
                format!("{} #{number}", s(p, "title")),
                sub,
                actions,
                Some(s(p, "html_url").to_owned()),
            ))
            .child(
                div()
                    .px(px(16.0))
                    .py(px(12.0))
                    .flex()
                    .flex_col()
                    .gap(px(16.0))
                    .child(checks)
                    .children(reviews)
                    .children(description)
                    .child(files)
                    .child(comment),
            )
            .into_any_element()
    }

    /// A changed file of a pull request: click it for GitHub's patch.
    fn file(&self, index: usize, file: &Value, cx: &mut Context<Self>) -> AnyElement {
        let name = s(file, "filename").to_owned();
        let open = self.open_files.contains(&name);
        let key = name.clone();
        let head = div()
            .id(("pr-file", index))
            .h(px(28.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .cursor_pointer()
            .hover(|style| style.text_color(color(theme::ACCENT_TEXT)))
            .on_click(cx.listener(move |this, _e, _w, cx| {
                if !this.open_files.remove(&key) {
                    this.open_files.insert(key.clone());
                }
                cx.notify();
            }))
            .child(
                div()
                    .w(px(10.0))
                    .text_color(color(theme::MUTED))
                    .child(if open { "▾" } else { "▸" }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family(theme::MONO)
                    .text_size(px(theme::TEXT_SM))
                    .child(name.clone()),
            )
            .children(
                (s(file, "status") != "modified").then(|| muted(s(file, "status").to_owned())),
            )
            .child(
                div()
                    .font_family(theme::MONO)
                    .text_size(px(theme::TEXT_XS))
                    .text_color(color(theme::OK))
                    .child(format!("+{}", n(file, "additions"))),
            )
            .child(
                div()
                    .font_family(theme::MONO)
                    .text_size(px(theme::TEXT_XS))
                    .text_color(color(theme::DANGER))
                    .child(format!("−{}", n(file, "deletions"))),
            );
        let patch = open.then(|| {
            let body: AnyElement = match file.get("patch").and_then(Value::as_str) {
                Some(patch) => div()
                    .py(px(4.0))
                    .children(patch.lines().take(800).map(|l| {
                        let mut row = div()
                            .px(px(8.0))
                            .whitespace_nowrap()
                            .font_family(theme::MONO)
                            .text_size(px(theme::TEXT_SM));
                        row = if l.starts_with('+') {
                            row.bg(color(0x0016_3d2a))
                        } else if l.starts_with('-') {
                            row.bg(color(0x0041_1f22))
                        } else if l.starts_with("@@") {
                            row.text_color(color(theme::MUTED))
                        } else {
                            row
                        };
                        row.child(if l.is_empty() {
                            " ".to_owned()
                        } else {
                            l.to_owned()
                        })
                    }))
                    .into_any_element(),
                None => div()
                    .p(px(8.0))
                    .child(muted(
                        "No text diff (binary, or too large for GitHub to show).",
                    ))
                    .into_any_element(),
            };
            div()
                .id(("pr-patch", index))
                .mb(px(8.0))
                .rounded(px(theme::RADIUS_CONTROL))
                .border_1()
                .border_color(color(theme::HAIRLINE))
                .bg(color(theme::CODE_BG))
                .overflow_x_scroll()
                .child(body)
        });
        div()
            .border_b_1()
            .border_color(color(theme::HAIRLINE))
            .child(head)
            .children(patch)
            .into_any_element()
    }
}

impl Render for Prs {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = time::now();
        let filter = div()
            .flex()
            .gap(px(4.0))
            .px(px(12.0))
            .py(px(8.0))
            .border_b_1()
            .border_color(color(theme::HAIRLINE))
            .child(pill(
                "prs-open",
                "Open",
                !self.closed,
                cx.listener(|this, _e, _w, cx| {
                    this.closed = false;
                    this.picked = None;
                    this.list = Load::Loading;
                    this.reload(cx);
                    cx.notify();
                }),
            ))
            .child(pill(
                "prs-closed",
                "Closed",
                self.closed,
                cx.listener(|this, _e, _w, cx| {
                    this.closed = true;
                    this.picked = None;
                    this.list = Load::Loading;
                    this.reload(cx);
                    cx.notify();
                }),
            ));
        let active = self.picked.as_ref().map(|d| d.number);
        let rows: Vec<AnyElement> = self.list.data().map_or_else(Vec::new, |prs| {
            prs.iter()
                .map(|p| {
                    let number = n(p, "number");
                    let draft = p.get("draft").and_then(Value::as_bool).unwrap_or(false);
                    list_row(
                        ("pr-row", usize::try_from(number).unwrap_or(0)),
                        active == Some(number),
                        pr_mark(p).into_any_element(),
                        s(p, "title").to_owned(),
                        format!(
                            "#{number} · {} · {} → {} · {}",
                            at(p, &["user", "login"]),
                            at(p, &["head", "ref"]),
                            at(p, &["base", "ref"]),
                            time::ago(s(p, "updated_at"), now)
                        ),
                        draft.then(|| {
                            div()
                                .px(px(4.0))
                                .rounded(px(4.0))
                                .bg(color(theme::RAISED))
                                .text_size(px(theme::TEXT_XS))
                                .text_color(color(theme::MUTED))
                                .child("Draft")
                                .into_any_element()
                        }),
                        cx.listener(move |this, _e, _w, cx| this.open(number, cx)),
                    )
                    .into_any_element()
                })
                .collect()
        });
        let empty = if self.list.data().is_some_and(Vec::is_empty) {
            format!(
                "No {} pull requests.",
                if self.closed { "closed" } else { "open" }
            )
        } else {
            "Pick a pull request.".to_owned()
        };
        let listing = div()
            .flex()
            .flex_col()
            .child(filter)
            .children(self.list.error().map(problem))
            .children(matches!(self.list, Load::Loading).then(loading))
            .children(rows);
        let detail = self.picked.as_ref().map(|d| self.detail(d, cx));
        split(listing, detail, empty, "prs")
    }
}

// ── Actions ───────────────────────────────────────────────────────────────────────────────────────────────────

fn repos_file() -> std::path::PathBuf {
    vivianne_ai::paths::viv_home_path(&["agent", "git_action_repos.json"])
}

/// The repositories added beside a project's own, kept per folder.
fn load_repos(root: &str) -> Vec<String> {
    let all: HashMap<String, Vec<String>> = std::fs::read_to_string(repos_file())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    all.get(root).cloned().unwrap_or_default()
}

fn save_repos(root: &str, repos: &[String]) {
    let mut all: HashMap<String, Vec<String>> = std::fs::read_to_string(repos_file())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    all.insert(root.to_owned(), repos.to_vec());
    if let Ok(text) = serde_json::to_string(&all) {
        let _ = std::fs::write(repos_file(), text);
    }
}

/// `owner/repo` from what was typed or pasted.
#[must_use]
pub fn parse_repo(text: &str) -> Option<String> {
    let t = text
        .trim()
        .trim_start_matches("https://github.com/")
        .trim_start_matches("http://github.com/");
    let t = t.strip_suffix(".git").unwrap_or(t);
    let valid = |p: &str| {
        !p.is_empty()
            && p.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    };
    let (owner, name) = t.split_once('/')?;
    (valid(owner) && valid(name)).then(|| format!("{owner}/{name}"))
}

struct RunDetail {
    id: i64,
    run: Load<Value>,
    jobs: Load<Vec<Value>>,
    /// The job whose steps and log are open.
    open_job: Option<i64>,
    logs: HashMap<i64, Load<String>>,
}

pub struct Actions {
    root: String,
    own: Option<String>,
    extra: Vec<String>,
    /// The repository shown; `None` is the project's own.
    target: Option<String>,
    workflow: Option<String>,
    runs: Load<Vec<Value>>,
    picked: Option<RunDetail>,
    act: Act,
    adding: Entity<InputState>,
    _adding: Subscription,
}

impl EventEmitter<HubEvent> for Actions {}

impl Actions {
    pub fn new(root: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let adding = cx.new(|cx| InputState::new(window, cx).placeholder("+ owner/repo"));
        let sub = cx.subscribe_in(
            &adding,
            window,
            |this, input, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. })
                    && let Some(repo) = parse_repo(&input.read(cx).value())
                {
                    if !this.extra.contains(&repo) {
                        this.extra.push(repo.clone());
                        save_repos(&this.root, &this.extra);
                    }
                    input.update(cx, |i, cx| i.set_value("", window, cx));
                    this.pick_repo(Some(repo), cx);
                }
            },
        );
        let mut this = Self {
            extra: load_repos(&root),
            root,
            own: None,
            target: None,
            workflow: None,
            runs: Load::Loading,
            picked: None,
            act: Act::default(),
            adding,
            _adding: sub,
        };
        this.reload(cx);
        let dir = this.root.clone();
        let view = cx.entity().downgrade();
        crate::backend::run(
            cx,
            async move { vivianne_tools::git::github::repo(&dir).await },
            move |repo, cx| {
                view.update(cx, |this, cx| {
                    this.own = repo.ok();
                    cx.notify();
                })
                .ok();
            },
        );
        // Runs that are still going are read again while the tab is open; the loop ends with the tab.
        cx.spawn(async move |this, cx| {
            let mut tick = 0_u32;
            loop {
                cx.background_executor().timer(Duration::from_secs(5)).await;
                tick += 1;
                let gone = this
                    .update(cx, |this, cx| {
                        if let Some(d) = this
                            .picked
                            .as_ref()
                            .filter(|d| d.run.data().is_some_and(|r| s(r, "status") != "completed"))
                        {
                            let id = d.id;
                            this.open(id, false, cx);
                        }
                        if tick.is_multiple_of(2)
                            && this.runs.data().is_some_and(|runs| {
                                runs.iter().any(|r| s(r, "status") != "completed")
                            })
                        {
                            this.reload(cx);
                        }
                    })
                    .is_err();
                if gone {
                    return;
                }
            }
        })
        .detach();
        this
    }

    /// `{repo}` becomes the chosen repository when it is not the project's own.
    fn path(&self, path: &str) -> String {
        self.target
            .as_ref()
            .map_or_else(|| path.to_owned(), |t| path.replace("{repo}", t))
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let (root, path, target) = (
            self.root.clone(),
            self.path("/repos/{repo}/actions/runs?per_page=60"),
            self.target.clone(),
        );
        ui::call(cx, &root, path, "GET", None, move |this, answer, cx| {
            if this.target == target {
                this.runs = Load::from(
                    answer.map(|v| list(v.get("workflow_runs").unwrap_or(&Value::Null)).to_vec()),
                );
                cx.notify();
            }
        });
    }

    fn pick_repo(&mut self, target: Option<String>, cx: &mut Context<Self>) {
        self.target = target;
        self.workflow = None;
        self.picked = None;
        self.runs = Load::Loading;
        self.reload(cx);
        cx.notify();
    }

    /// Reads a run and its jobs; `reset` starts the detail over (a newly picked run).
    fn open(&mut self, id: i64, reset: bool, cx: &mut Context<Self>) {
        if reset || self.picked.as_ref().is_none_or(|d| d.id != id) {
            self.picked = Some(RunDetail {
                id,
                run: Load::Loading,
                jobs: Load::Loading,
                open_job: None,
                logs: HashMap::new(),
            });
            self.act = Act::default();
        }
        let root = self.root.clone();
        let (run_path, jobs_path) = (
            self.path(&format!("/repos/{{repo}}/actions/runs/{id}")),
            self.path(&format!(
                "/repos/{{repo}}/actions/runs/{id}/jobs?per_page=100"
            )),
        );
        ui::call(cx, &root, run_path, "GET", None, move |this, answer, cx| {
            if let Some(d) = this.picked.as_mut().filter(|d| d.id == id) {
                d.run = Load::from(answer);
                cx.notify();
            }
        });
        ui::call(
            cx,
            &root,
            jobs_path,
            "GET",
            None,
            move |this, answer, cx| {
                let Some(d) = this.picked.as_mut().filter(|d| d.id == id) else {
                    return;
                };
                d.jobs = Load::from(
                    answer.map(|v| list(v.get("jobs").unwrap_or(&Value::Null)).to_vec()),
                );
                // A failed job is open from the start: where it failed is what is wanted.
                if d.open_job.is_none()
                    && let Some(job) = d
                        .jobs
                        .data()
                        .and_then(|jobs| jobs.iter().find(|j| s(j, "conclusion") == "failure"))
                        .map(|j| n(j, "id"))
                {
                    this.open_job(job, cx);
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    fn open_job(&mut self, job: i64, cx: &mut Context<Self>) {
        let Some(d) = self.picked.as_mut() else {
            return;
        };
        let run = d.id;
        d.open_job = if d.open_job == Some(job) {
            None
        } else {
            Some(job)
        };
        let completed = d
            .jobs
            .data()
            .and_then(|jobs| jobs.iter().find(|j| n(j, "id") == job))
            .is_some_and(|j| s(j, "status") == "completed");
        if d.open_job == Some(job) && completed && !d.logs.contains_key(&job) {
            d.logs.insert(job, Load::Loading);
            let (root, path) = (
                self.root.clone(),
                self.path(&format!("/repos/{{repo}}/actions/jobs/{job}/logs")),
            );
            let view = cx.entity().downgrade();
            crate::backend::run(
                cx,
                ui::github(root, path, "GET", None, true),
                move |answer, cx| {
                    view.update(cx, |this, cx| {
                        if let Some(d) = this.picked.as_mut().filter(|d| d.id == run) {
                            d.logs.insert(
                                job,
                                Load::from(answer.map(|v| v.as_str().unwrap_or("").to_owned())),
                            );
                            cx.notify();
                        }
                    })
                    .ok();
                },
            );
        }
        cx.notify();
    }

    fn change(&mut self, label: &'static str, path: String, cx: &mut Context<Self>) {
        let Some(id) = self.picked.as_ref().map(|d| d.id) else {
            return;
        };
        if !self.act.click(label, false) {
            return;
        }
        let (root, path) = (self.root.clone(), self.path(&path));
        ui::call(cx, &root, path, "POST", None, move |this, answer, cx| {
            this.act.done(&answer);
            if answer.is_ok() {
                this.open(id, false, cx);
                this.reload(cx);
                cx.emit(HubEvent::Changed);
            }
            cx.notify();
        });
        cx.notify();
    }

    fn detail(&self, d: &RunDetail, cx: &mut Context<Self>) -> AnyElement {
        if let Some(e) = d.run.error() {
            return problem(e).into_any_element();
        }
        let Some(r) = d.run.data() else {
            return loading();
        };
        let id = d.id;
        let now = time::now();
        let live = s(r, "status") != "completed";
        let title = if s(r, "display_title").is_empty() {
            s(r, "name")
        } else {
            s(r, "display_title")
        };
        let sub = div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .flex_wrap()
            .child(status_mark(s(r, "status"), s(r, "conclusion")))
            .child(format!(
                "{} #{} · {} on",
                s(r, "name"),
                n(r, "run_number"),
                s(r, "event")
            ))
            .child(ui::mono(s(r, "head_branch").to_owned()))
            .child(format!(
                "· {} · {} · {}",
                s(r, "head_sha").chars().take(7).collect::<String>(),
                time::ago(s(r, "created_at"), now),
                time::duration(
                    r.get("run_started_at").and_then(Value::as_str),
                    if live {
                        None
                    } else {
                        r.get("updated_at").and_then(Value::as_str)
                    },
                    now
                )
            ));
        let mut actions = Vec::new();
        if live {
            let path = format!("/repos/{{repo}}/actions/runs/{id}/cancel");
            actions.push(self.act.button(
                "run-cancel",
                "Cancel",
                true,
                cx.listener(move |this, _e, _w, cx| this.change("Cancel", path.clone(), cx)),
            ));
        } else {
            if s(r, "conclusion") == "failure" {
                let path = format!("/repos/{{repo}}/actions/runs/{id}/rerun-failed-jobs");
                actions.push(self.act.button(
                    "run-rerun-failed",
                    "Re-run failed jobs",
                    false,
                    cx.listener(move |this, _e, _w, cx| {
                        this.change("Re-run failed jobs", path.clone(), cx)
                    }),
                ));
            }
            let path = format!("/repos/{{repo}}/actions/runs/{id}/rerun");
            actions.push(self.act.button(
                "run-rerun",
                "Re-run all",
                false,
                cx.listener(move |this, _e, _w, cx| this.change("Re-run all", path.clone(), cx)),
            ));
        }
        let jobs: Vec<AnyElement> = match &d.jobs {
            Load::Done(jobs) => jobs.iter().map(|j| Self::job(d, j, now, cx)).collect(),
            Load::Failed(e) => vec![problem(e).into_any_element()],
            Load::Loading => vec![loading()],
        };
        div()
            .flex()
            .flex_col()
            .child(detail_head(
                title.to_owned(),
                sub,
                actions,
                Some(s(r, "html_url").to_owned()),
            ))
            .child(
                div()
                    .px(px(16.0))
                    .py(px(12.0))
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .children(jobs),
            )
            .into_any_element()
    }

    fn job(d: &RunDetail, job: &Value, now: i64, cx: &mut Context<Self>) -> AnyElement {
        let id = n(job, "id");
        let open = d.open_job == Some(id);
        let head = div()
            .id(("job", usize::try_from(id).unwrap_or(0)))
            .h(px(32.0))
            .px(px(12.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .cursor_pointer()
            .hover(|style| style.bg(color(theme::SURFACE)))
            .on_click(cx.listener(move |this, _e, _w, cx| this.open_job(id, cx)))
            .child(status_mark(s(job, "status"), s(job, "conclusion")))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(theme::TEXT_SM))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(s(job, "name").to_owned()),
            )
            .child(muted(time::duration(
                job.get("started_at").and_then(Value::as_str),
                job.get("completed_at").and_then(Value::as_str),
                now,
            )));
        let body = open.then(|| {
            let steps = div().px(px(12.0)).py(px(4.0)).flex().flex_col().children(
                list(job.get("steps").unwrap_or(&Value::Null))
                    .iter()
                    .map(|st| {
                        div()
                            .h(px(20.0))
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(status_mark(s(st, "status"), s(st, "conclusion")))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(theme::TEXT_SM))
                                    .child(s(st, "name").to_owned()),
                            )
                            .child(
                                div()
                                    .text_size(px(theme::TEXT_XS))
                                    .text_color(color(theme::FAINT))
                                    .child(time::duration(
                                        st.get("started_at").and_then(Value::as_str),
                                        st.get("completed_at").and_then(Value::as_str),
                                        now,
                                    )),
                            )
                    }),
            );
            let log = (s(job, "status") == "completed").then(|| match d.logs.get(&id) {
                Some(Load::Done(text)) => {
                    // From the end: where it failed is usually the last screen.
                    let lines: Vec<String> =
                        text.lines().map(|l| strip_ansi(strip_stamp(l))).collect();
                    let tail = lines[lines.len().saturating_sub(400)..].join("\n");
                    div()
                        .id(("job-log", usize::try_from(id).unwrap_or(0)))
                        .max_h(px(420.0))
                        .overflow_y_scroll()
                        .border_t_1()
                        .border_color(color(theme::HAIRLINE))
                        .px(px(12.0))
                        .py(px(8.0))
                        .font_family(theme::MONO)
                        .text_size(px(theme::TEXT_SM))
                        .child(if tail.is_empty() {
                            "No log.".to_owned()
                        } else {
                            tail
                        })
                        .into_any_element()
                }
                Some(Load::Failed(e)) => div()
                    .px(px(12.0))
                    .pb(px(8.0))
                    .text_size(px(theme::TEXT_XS))
                    .text_color(color(theme::DANGER))
                    .child(e.clone())
                    .into_any_element(),
                _ => div()
                    .px(px(12.0))
                    .pb(px(8.0))
                    .child(muted("Loading the log…"))
                    .into_any_element(),
            });
            div()
                .border_t_1()
                .border_color(color(theme::HAIRLINE))
                .bg(color(theme::CODE_BG))
                .child(steps)
                .children(log)
        });
        div()
            .rounded(px(theme::RADIUS_SURFACE))
            .border_1()
            .border_color(color(theme::BORDER))
            .overflow_hidden()
            .child(head)
            .children(body)
            .into_any_element()
    }
}

/// A log line without the timestamp GitHub puts first.
fn strip_stamp(line: &str) -> &str {
    let bytes = line.as_bytes();
    // `2026-11-20T10:00:00.1234567Z `
    if bytes.len() > 28
        && bytes[4] == b'-'
        && bytes[10] == b'T'
        && let Some(end) = line.find("Z ")
    {
        return &line[end + 2..];
    }
    line
}

/// A log line without the colour codes the runner put in it.
fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            // `ESC [ ... letter`
            for end in chars.by_ref() {
                if end.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

impl Render for Actions {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = time::now();
        let runs = self.runs.data().cloned().unwrap_or_default();
        let mut names: Vec<String> = Vec::new();
        for r in &runs {
            if !names.iter().any(|n| n == s(r, "name")) {
                names.push(s(r, "name").to_owned());
            }
        }
        let repos = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(4.0))
            .px(px(12.0))
            .pt(px(8.0))
            .child(pill(
                "repo-own",
                self.own
                    .clone()
                    .unwrap_or_else(|| "This repository".to_owned()),
                self.target.is_none(),
                cx.listener(|this, _e, _w, cx| this.pick_repo(None, cx)),
            ))
            .children(self.extra.iter().enumerate().map(|(i, repo)| {
                let (pick, drop) = (repo.clone(), repo.clone());
                div()
                    .flex()
                    .items_center()
                    .child(pill(
                        ("repo", i),
                        repo.clone(),
                        self.target.as_ref() == Some(repo),
                        cx.listener(move |this, _e, _w, cx| this.pick_repo(Some(pick.clone()), cx)),
                    ))
                    .child(
                        div()
                            .id(("repo-drop", i))
                            .w(px(16.0))
                            .text_size(px(theme::TEXT_SM))
                            .text_color(color(theme::FAINT))
                            .cursor_pointer()
                            .hover(|style| style.text_color(color(theme::TEXT)))
                            .on_click(cx.listener(move |this, _e, _w, cx| {
                                this.extra.retain(|r| *r != drop);
                                save_repos(&this.root, &this.extra);
                                if this.target.as_ref() == Some(&drop) {
                                    this.pick_repo(None, cx);
                                }
                                cx.notify();
                            }))
                            .child("×"),
                    )
            }))
            .child(div().w(px(120.0)).child(Input::new(&self.adding).small()));
        let workflows = div()
            .flex()
            .flex_wrap()
            .gap(px(4.0))
            .px(px(12.0))
            .py(px(8.0))
            .border_b_1()
            .border_color(color(theme::HAIRLINE))
            .child(pill(
                "wf-all",
                "All workflows",
                self.workflow.is_none(),
                cx.listener(|this, _e, _w, cx| {
                    this.workflow = None;
                    cx.notify();
                }),
            ))
            .children(names.iter().enumerate().map(|(i, name)| {
                let pick = name.clone();
                pill(
                    ("wf", i),
                    name.clone(),
                    self.workflow.as_ref() == Some(name),
                    cx.listener(move |this, _e, _w, cx| {
                        this.workflow = Some(pick.clone());
                        cx.notify();
                    }),
                )
            }));
        let active = self.picked.as_ref().map(|d| d.id);
        let shown: Vec<AnyElement> = runs
            .iter()
            .filter(|r| self.workflow.as_deref().is_none_or(|w| s(r, "name") == w))
            .map(|r| {
                let id = n(r, "id");
                let title = if s(r, "display_title").is_empty() {
                    s(r, "name")
                } else {
                    s(r, "display_title")
                };
                list_row(
                    ("run", usize::try_from(id).unwrap_or(0)),
                    active == Some(id),
                    status_mark(s(r, "status"), s(r, "conclusion")),
                    title.to_owned(),
                    format!(
                        "{} #{} · {} · {} · {}",
                        s(r, "name"),
                        n(r, "run_number"),
                        s(r, "head_branch"),
                        s(r, "event"),
                        time::ago(s(r, "created_at"), now)
                    ),
                    None,
                    cx.listener(move |this, _e, _w, cx| this.open(id, true, cx)),
                )
                .into_any_element()
            })
            .collect();
        let empty = if shown.is_empty() && self.runs.data().is_some() {
            "No workflow runs here yet."
        } else {
            "Pick a run to see its jobs and logs."
        };
        let listing = div()
            .flex()
            .flex_col()
            .child(repos)
            .child(workflows)
            .children(self.runs.error().map(problem))
            .children(matches!(self.runs, Load::Loading).then(loading))
            .children(shown);
        let detail = self.picked.as_ref().map(|d| self.detail(d, cx));
        split(listing, detail, empty, "actions")
    }
}

// ── Issues ────────────────────────────────────────────────────────────────────────────────────────────────────

struct IssueDetail {
    number: i64,
    issue: Load<Value>,
    comments: Load<Vec<Value>>,
}

pub struct Issues {
    root: String,
    closed: bool,
    list: Load<Vec<Value>>,
    picked: Option<IssueDetail>,
    act: Act,
    title: Entity<InputState>,
    comment: Entity<TextareaState>,
}

impl EventEmitter<HubEvent> for Issues {}

impl Issues {
    pub fn new(root: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let title = cx.new(|cx| InputState::new(window, cx).placeholder("New issue title"));
        let comment = textarea(2, "Leave a comment", window, cx);
        let mut this = Self {
            root,
            closed: false,
            list: Load::Loading,
            picked: None,
            act: Act::default(),
            title,
            comment,
        };
        this.reload(cx);
        this
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let (state, want) = (if self.closed { "closed" } else { "open" }, self.closed);
        let root = self.root.clone();
        ui::call(
            cx,
            &root,
            format!("/repos/{{repo}}/issues?state={state}&per_page=50&sort=updated"),
            "GET",
            None,
            move |this, answer, cx| {
                if this.closed == want {
                    // GitHub lists pull requests among the issues.
                    this.list = Load::from(answer.map(|v| {
                        list(&v)
                            .iter()
                            .filter(|i| i.get("pull_request").is_none())
                            .cloned()
                            .collect()
                    }));
                    cx.notify();
                }
            },
        );
    }

    fn open(&mut self, number: i64, cx: &mut Context<Self>) {
        self.picked = Some(IssueDetail {
            number,
            issue: Load::Loading,
            comments: Load::Loading,
        });
        self.act = Act::default();
        let root = self.root.clone();
        ui::call(
            cx,
            &root,
            format!("/repos/{{repo}}/issues/{number}"),
            "GET",
            None,
            move |this, answer, cx| {
                if let Some(d) = this.picked.as_mut().filter(|d| d.number == number) {
                    d.issue = Load::from(answer);
                    cx.notify();
                }
            },
        );
        ui::call(
            cx,
            &root,
            format!("/repos/{{repo}}/issues/{number}/comments?per_page=100"),
            "GET",
            None,
            move |this, answer, cx| {
                if let Some(d) = this.picked.as_mut().filter(|d| d.number == number) {
                    d.comments = Load::from(answer.map(|v| list(&v).to_vec()));
                    cx.notify();
                }
            },
        );
        cx.notify();
    }

    fn change(
        &mut self,
        label: &'static str,
        path: String,
        method: &'static str,
        body: Value,
        open_new: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.act.click(label, false) {
            return;
        }
        let root = self.root.clone();
        ui::call(
            cx,
            &root,
            path,
            method,
            Some(body),
            move |this, answer, cx| {
                this.act.done(&answer);
                // A new issue is opened; otherwise the one already open is read again.
                let made = answer
                    .as_ref()
                    .ok()
                    .map(|v| n(v, "number"))
                    .filter(|_| open_new);
                if answer.is_ok() {
                    this.reload(cx);
                    if let Some(number) = made.or_else(|| this.picked.as_ref().map(|d| d.number)) {
                        this.open(number, cx);
                    }
                    cx.emit(HubEvent::Changed);
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    fn detail(&self, d: &IssueDetail, cx: &mut Context<Self>) -> AnyElement {
        if let Some(e) = d.issue.error() {
            return problem(e).into_any_element();
        }
        let Some(i) = d.issue.data() else {
            return loading();
        };
        let number = d.number;
        let now = time::now();
        let open = s(i, "state") == "open";
        let path = format!("/repos/{{repo}}/issues/{number}");
        let action = if open {
            self.act.button(
                "issue-close",
                "Close",
                true,
                cx.listener(move |this, _e, _w, cx| {
                    this.change(
                        "Close",
                        path.clone(),
                        "PATCH",
                        json!({ "state": "closed" }),
                        false,
                        cx,
                    )
                }),
            )
        } else {
            self.act.button(
                "issue-reopen",
                "Reopen",
                false,
                cx.listener(move |this, _e, _w, cx| {
                    this.change(
                        "Reopen",
                        path.clone(),
                        "PATCH",
                        json!({ "state": "open" }),
                        false,
                        cx,
                    )
                }),
            )
        };
        let comments: Vec<AnyElement> = d.comments.data().map_or_else(Vec::new, |c| {
            c.iter()
                .map(|c| {
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .border_t_1()
                        .border_color(color(theme::HAIRLINE))
                        .pt(px(12.0))
                        .child(muted(format!(
                            "{} · {}",
                            at(c, &["user", "login"]),
                            time::ago(s(c, "created_at"), now)
                        )))
                        .child(markdown(
                            format!("issue-comment-{}", n(c, "id")),
                            s(c, "body"),
                        ))
                        .into_any_element()
                })
                .collect()
        });
        div()
            .flex()
            .flex_col()
            .child(detail_head(
                format!("{} #{number}", s(i, "title")),
                format!(
                    "{} · opened by {} {}",
                    if open { "Open" } else { "Closed" },
                    at(i, &["user", "login"]),
                    time::ago(s(i, "created_at"), now)
                ),
                vec![action],
                Some(s(i, "html_url").to_owned()),
            ))
            .child(
                div()
                    .px(px(16.0))
                    .py(px(12.0))
                    .flex()
                    .flex_col()
                    .gap(px(16.0))
                    .children(
                        (!s(i, "body").is_empty())
                            .then(|| markdown(format!("issue-{number}-body"), s(i, "body"))),
                    )
                    .children(comments)
                    .child(bordered(Textarea::new(&self.comment).appearance(false)))
                    .child(div().child(self.act.button(
                        "issue-comment",
                        "Comment",
                        false,
                        cx.listener(move |this, _e, window, cx| {
                            let text = this.comment.read(cx).value().trim().to_owned();
                            if text.is_empty() {
                                return;
                            }
                            this.comment.update(cx, |c, cx| c.set_value("", window, cx));
                            this.change(
                                "Comment",
                                format!("/repos/{{repo}}/issues/{number}/comments"),
                                "POST",
                                json!({ "body": text }),
                                false,
                                cx,
                            );
                        }),
                    ))),
            )
            .into_any_element()
    }
}

impl Render for Issues {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = time::now();
        let filter = div()
            .flex()
            .gap(px(4.0))
            .px(px(12.0))
            .py(px(8.0))
            .border_b_1()
            .border_color(color(theme::HAIRLINE))
            .child(pill(
                "issues-open",
                "Open",
                !self.closed,
                cx.listener(|this, _e, _w, cx| {
                    this.closed = false;
                    this.picked = None;
                    this.list = Load::Loading;
                    this.reload(cx);
                    cx.notify();
                }),
            ))
            .child(pill(
                "issues-closed",
                "Closed",
                self.closed,
                cx.listener(|this, _e, _w, cx| {
                    this.closed = true;
                    this.picked = None;
                    this.list = Load::Loading;
                    this.reload(cx);
                    cx.notify();
                }),
            ));
        let create = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(12.0))
            .py(px(8.0))
            .border_b_1()
            .border_color(color(theme::HAIRLINE))
            .child(div().flex_1().child(Input::new(&self.title).small()))
            .child(self.act.button(
                "issue-create",
                "Create",
                false,
                cx.listener(|this, _e, window, cx| {
                    let title = this.title.read(cx).value().trim().to_owned();
                    if title.is_empty() {
                        return;
                    }
                    this.title.update(cx, |t, cx| t.set_value("", window, cx));
                    this.change(
                        "Create",
                        "/repos/{repo}/issues".to_owned(),
                        "POST",
                        json!({ "title": title }),
                        true,
                        cx,
                    );
                }),
            ));
        let active = self.picked.as_ref().map(|d| d.number);
        let rows: Vec<AnyElement> = self.list.data().map_or_else(Vec::new, |issues| {
            issues
                .iter()
                .map(|i| {
                    let number = n(i, "number");
                    let comments = n(i, "comments");
                    let labels = list(i.get("labels").unwrap_or(&Value::Null));
                    list_row(
                        ("issue-row", usize::try_from(number).unwrap_or(0)),
                        active == Some(number),
                        ui::dot(if s(i, "state") == "open" {
                            theme::OK
                        } else {
                            ui::MERGED
                        })
                        .into_any_element(),
                        s(i, "title").to_owned(),
                        format!(
                            "#{number} · {} · {comments} comment{} · {}",
                            at(i, &["user", "login"]),
                            if comments == 1 { "" } else { "s" },
                            time::ago(s(i, "updated_at"), now)
                        ),
                        (!labels.is_empty()).then(|| {
                            div()
                                .flex()
                                .gap(px(4.0))
                                .children(labels.iter().take(2).map(|l| {
                                    div()
                                        .px(px(4.0))
                                        .rounded(px(4.0))
                                        .bg(color(theme::RAISED))
                                        .text_size(px(theme::TEXT_XS))
                                        .text_color(color(theme::MUTED))
                                        .child(s(l, "name").to_owned())
                                }))
                                .into_any_element()
                        }),
                        cx.listener(move |this, _e, _w, cx| this.open(number, cx)),
                    )
                    .into_any_element()
                })
                .collect()
        });
        let empty = if self.list.data().is_some_and(Vec::is_empty) {
            format!("No {} issues.", if self.closed { "closed" } else { "open" })
        } else {
            "Pick an issue.".to_owned()
        };
        let listing = div()
            .flex()
            .flex_col()
            .child(filter)
            .child(create)
            .children(self.list.error().map(problem))
            .children(matches!(self.list, Load::Loading).then(loading))
            .children(rows);
        let detail = self.picked.as_ref().map(|d| self.detail(d, cx));
        split(listing, detail, empty, "issues")
    }
}

// ── Releases ──────────────────────────────────────────────────────────────────────────────────────────────────

pub struct Releases {
    list: Load<Vec<Value>>,
}

impl Releases {
    pub fn new(root: String, cx: &mut Context<Self>) -> Self {
        ui::call(
            cx,
            &root,
            "/repos/{repo}/releases?per_page=30".to_owned(),
            "GET",
            None,
            |this: &mut Self, answer, cx| {
                this.list = Load::from(answer.map(|v| list(&v).to_vec()));
                cx.notify();
            },
        );
        Self {
            list: Load::Loading,
        }
    }
}

impl Render for Releases {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let now = time::now();
        let body: AnyElement = match &self.list {
            Load::Loading => loading(),
            Load::Failed(e) => problem(e).into_any_element(),
            Load::Done(releases) if releases.is_empty() => div()
                .p(px(16.0))
                .child(muted("No releases yet."))
                .into_any_element(),
            Load::Done(releases) => div()
                .max_w(px(880.0))
                .px(px(16.0))
                .py(px(12.0))
                .flex()
                .flex_col()
                .gap(px(16.0))
                .children(releases.iter().map(|r| {
                    let url = s(r, "html_url").to_owned();
                    let published = if s(r, "published_at").is_empty() {
                        s(r, "created_at")
                    } else {
                        s(r, "published_at")
                    };
                    let tag = |text: &'static str, hex: u32| {
                        div()
                            .px(px(4.0))
                            .rounded(px(4.0))
                            .bg(color(theme::RAISED))
                            .text_size(px(theme::TEXT_XS))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(color(hex))
                            .child(text)
                    };
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .pb(px(16.0))
                        .border_b_1()
                        .border_color(color(theme::HAIRLINE))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .text_size(px(14.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(
                                            if s(r, "name").is_empty() {
                                                s(r, "tag_name")
                                            } else {
                                                s(r, "name")
                                            }
                                            .to_owned(),
                                        ),
                                )
                                .child(
                                    div()
                                        .px(px(4.0))
                                        .rounded(px(4.0))
                                        .border_1()
                                        .border_color(color(theme::BORDER))
                                        .font_family(theme::MONO)
                                        .text_size(px(theme::TEXT_XS))
                                        .text_color(color(theme::MUTED))
                                        .child(s(r, "tag_name").to_owned()),
                                )
                                .children(
                                    r.get("prerelease")
                                        .and_then(Value::as_bool)
                                        .unwrap_or(false)
                                        .then(|| tag("Pre-release", theme::WARN)),
                                )
                                .children(
                                    r.get("draft")
                                        .and_then(Value::as_bool)
                                        .unwrap_or(false)
                                        .then(|| tag("Draft", theme::MUTED)),
                                )
                                .child(div().flex_1())
                                .child(muted(time::ago(published, now)))
                                .child(
                                    Button::new(SharedString::from(format!(
                                        "release-{}",
                                        n(r, "id")
                                    )))
                                    .ghost()
                                    .xsmall()
                                    .label("Open")
                                    .on_click(move |_e, _w, cx| cx.open_url(&url)),
                                ),
                        )
                        .children((!s(r, "body").is_empty()).then(|| {
                            markdown(format!("release-{}-body", n(r, "id")), s(r, "body"))
                        }))
                        .child(
                            div().flex().flex_wrap().gap(px(12.0)).children(
                                list(r.get("assets").unwrap_or(&Value::Null))
                                    .iter()
                                    .map(|a| {
                                        let url = s(a, "browser_download_url").to_owned();
                                        div()
                                            .id(SharedString::from(format!("asset-{}", n(a, "id"))))
                                            .font_family(theme::MONO)
                                            .text_size(px(theme::TEXT_XS))
                                            .text_color(color(theme::ACCENT_TEXT))
                                            .cursor_pointer()
                                            .on_click(move |_e, _w, cx| cx.open_url(&url))
                                            .child(format!(
                                                "{} ({} MB · {}↓)",
                                                s(a, "name"),
                                                n(a, "size") / 1024 / 1024,
                                                n(a, "download_count")
                                            ))
                                    }),
                            ),
                        )
                }))
                .into_any_element(),
        };
        div()
            .id("releases")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repository_is_read_from_what_was_pasted() {
        assert_eq!(
            parse_repo("https://github.com/o/r.git").as_deref(),
            Some("o/r")
        );
        assert_eq!(parse_repo(" o/r ").as_deref(), Some("o/r"));
        assert_eq!(parse_repo("o/r/extra"), None);
        assert_eq!(parse_repo("not a repo"), None);
    }

    #[test]
    fn log_lines_lose_their_colour_codes() {
        assert_eq!(
            strip_ansi("\u{1b}[0m\u{1b}[31m-    let x\u{1b}[0m"),
            "-    let x"
        );
        assert_eq!(strip_ansi("plain"), "plain");
    }

    #[test]
    fn log_lines_lose_their_timestamp() {
        assert_eq!(
            strip_stamp("2026-11-20T10:00:00.1234567Z build ok"),
            "build ok"
        );
        assert_eq!(strip_stamp("plain"), "plain");
    }
}
