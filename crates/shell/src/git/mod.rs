//! The Git tab: the project's repository on one page. Its branch and GitHub remote, what changed with the diff
//! review, pull requests with their checks, Actions runs with their jobs and logs, issues, releases, and the
//! history. GitHub comes through the same token the agent uses.

mod changes;
mod compose;
mod hub;
mod status;
mod ui;

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, Entity, EventEmitter, FontWeight, Subscription, Window, div, px,
};
use vivianne_tools::git::{self, Attention, Branch, Commit, Direction, Remote, Summary, time};

use self::changes::ChangesView;
use self::compose::{Compose, Kind};
use self::hub::{Actions, HubEvent, Issues, Prs, Releases};
use self::status::{BranchPr, Checks, Github, PrState};
use self::ui::{faint, mono, muted};
use crate::theme::{self, color};

/// How often the local state is read, and how many of those reads between reads of GitHub.
const POLL: Duration = Duration::from_secs(5);
const GITHUB_EVERY: u32 = 9;

/// What the page asks the app to do.
#[derive(Debug, Clone)]
pub enum GitEvent {
    /// Back to the chat (or Home).
    Back,
    /// Hand the agent a git job in the chat.
    AskAgent(String),
}

/// The repository at a glance, for the rail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Badge {
    pub ahead: u32,
    pub behind: u32,
    /// This branch's pull request number and what its checks say.
    pub pr: Option<(i64, &'static str)>,
    pub attention: Option<Attention>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Changes,
    PullRequests,
    Actions,
    Issues,
    Releases,
    History,
}

const TABS: [(Tab, &str); 6] = [
    (Tab::Changes, "Changes"),
    (Tab::PullRequests, "Pull requests"),
    (Tab::Actions, "Actions"),
    (Tab::Issues, "Issues"),
    (Tab::Releases, "Releases"),
    (Tab::History, "History"),
];

/// The open tab's content, created when it is first shown and dropped when another is.
enum Panel {
    Changes(Entity<ChangesView>),
    Prs(Entity<Prs>),
    Actions(Entity<Actions>),
    Issues(Entity<Issues>),
    Releases(Entity<Releases>),
    History,
}

pub struct GitView {
    root: String,
    summary: Option<Summary>,
    remote: Option<Remote>,
    commits: Vec<Commit>,
    branches: Vec<Branch>,
    github: Github,
    loading: bool,
    tab: Tab,
    panel: Option<Panel>,
    /// Listens to the open GitHub tab; dropped with it.
    _hub: Option<Subscription>,
    busy: Option<&'static str>,
    /// What the last move said, and whether it was a failure.
    note: Option<(String, bool)>,
}

impl EventEmitter<GitEvent> for GitView {}

impl GitView {
    pub fn new(root: PathBuf, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            root: root.display().to_string(),
            summary: None,
            remote: None,
            commits: Vec::new(),
            branches: Vec::new(),
            github: Github::default(),
            loading: true,
            tab: Tab::Changes,
            panel: None,
            _hub: None,
            busy: None,
            note: None,
        };
        view.refresh(cx);
        // Kept fresh: the local state every few seconds, GitHub less often.
        cx.spawn(async move |this, cx| {
            let mut tick = 0_u32;
            loop {
                cx.background_executor().timer(POLL).await;
                tick += 1;
                if this
                    .update(cx, |view, cx| {
                        view.refresh(cx);
                        if tick.is_multiple_of(GITHUB_EVERY) {
                            view.refresh_github(cx);
                        }
                    })
                    .is_err()
                {
                    return;
                }
            }
        })
        .detach();
        view
    }

    /// What needs a look, for the title bar's Git tab.
    pub fn attention(&self) -> Option<Attention> {
        self.summary.as_ref()?.attention(
            self.github
                .pr
                .as_ref()
                .is_some_and(|pr| pr.checks == Checks::Failing),
        )
    }

    /// What the rail's Version control section shows beside the branch.
    pub fn badge(&self) -> Option<Badge> {
        let s = self.summary.as_ref()?;
        let pr = self.github.pr.as_ref().map(|pr| {
            let checks = match pr.checks {
                Checks::Failing => "checks failing",
                Checks::Running => "checks running",
                Checks::Passing => "checks passing",
                Checks::None => "",
            };
            (pr.number, checks)
        });
        Some(Badge {
            ahead: s.ahead,
            behind: s.behind,
            pr,
            attention: self.attention(),
        })
    }

    /// Reads the repository's local state; GitHub is read again when the branch moves.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let view = cx.entity().downgrade();
        crate::backend::run(
            cx,
            async move {
                (
                    git::summary(&root).await,
                    git::remote(&root).await,
                    git::log(&root, 50).await,
                    git::branches(&root).await,
                )
            },
            move |(summary, remote, commits, branches), cx| {
                view.update(cx, |this, cx| {
                    let moved = this.summary.as_ref().map(|s| &s.branch)
                        != summary.as_ref().map(|s| &s.branch);
                    this.loading = false;
                    let changed = this.summary != summary
                        || this.remote != remote
                        || this.commits != commits
                        || this.branches != branches;
                    (this.summary, this.remote, this.commits, this.branches) =
                        (summary, remote, commits, branches);
                    if moved {
                        this.refresh_github(cx);
                    }
                    if changed {
                        cx.notify();
                    }
                })
                .ok();
            },
        );
    }

    pub fn refresh_github(&mut self, cx: &mut Context<Self>) {
        let Some(branch) = self.summary.as_ref().map(|s| s.branch.clone()) else {
            return;
        };
        let (root, view) = (self.root.clone(), cx.entity().downgrade());
        crate::backend::run(cx, status::fetch(root, branch), move |github, cx| {
            view.update(cx, |this, cx| {
                this.github = github;
                cx.notify();
            })
            .ok();
        });
    }

    fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        self.panel = None;
        self._hub = None;
        cx.notify();
    }

    /// Everything is read again: the local state, GitHub, and the open tab.
    fn reload(&mut self, cx: &mut Context<Self>) {
        self.panel = None;
        self._hub = None;
        self.loading = true;
        self.refresh(cx);
        self.refresh_github(cx);
        cx.notify();
    }

    fn panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let root = self.root.clone();
        let panel = self.panel.get_or_insert_with(|| match self.tab {
            Tab::Changes => Panel::Changes(cx.new(|cx| ChangesView::new(root, cx))),
            Tab::PullRequests => {
                let prs = cx.new(|cx| Prs::new(root, window, cx));
                self._hub = Some(cx.subscribe(&prs, Self::on_hub));
                Panel::Prs(prs)
            }
            Tab::Actions => {
                let actions = cx.new(|cx| Actions::new(root, window, cx));
                self._hub = Some(cx.subscribe(&actions, Self::on_hub));
                Panel::Actions(actions)
            }
            Tab::Issues => {
                let issues = cx.new(|cx| Issues::new(root, window, cx));
                self._hub = Some(cx.subscribe(&issues, Self::on_hub));
                Panel::Issues(issues)
            }
            Tab::Releases => Panel::Releases(cx.new(|cx| Releases::new(root, cx))),
            Tab::History => Panel::History,
        });
        match panel {
            Panel::Changes(v) => v.clone().into_any_element(),
            Panel::Prs(v) => v.clone().into_any_element(),
            Panel::Actions(v) => v.clone().into_any_element(),
            Panel::Issues(v) => v.clone().into_any_element(),
            Panel::Releases(v) => v.clone().into_any_element(),
            Panel::History => self.history(),
        }
    }

    fn on_hub<T: 'static>(&mut self, _hub: Entity<T>, event: &HubEvent, cx: &mut Context<Self>) {
        match event {
            HubEvent::Changed => self.refresh_github(cx),
            HubEvent::Checkout { branch, pr } => self.checkout(branch.clone(), Some(*pr), cx),
        }
    }

    /// Switches to a branch, or to a pull request's head. A branch another folder already has stays there.
    fn checkout(&mut self, branch: String, pr: Option<u64>, cx: &mut Context<Self>) {
        let (root, view) = (self.root.clone(), cx.entity().downgrade());
        self.busy = Some("Check out");
        self.note = None;
        cx.notify();
        let name = branch.clone();
        crate::backend::run(
            cx,
            async move { git::checkout(&root, &branch, pr).await },
            move |answer, cx| {
                view.update(cx, |this, cx| {
                    this.busy = None;
                    this.note = Some(match answer {
                        Ok(dir) if dir == this.root => (format!("Checked out {name}"), false),
                        Ok(dir) => (
                            format!("{name} is checked out in its own folder: {dir}"),
                            false,
                        ),
                        Err(e) => (e, true),
                    });
                    this.reload(cx);
                })
                .ok();
            },
        );
    }

    /// Pull, push or both, whichever the branch needs.
    fn sync(&mut self, label: &'static str, direction: Direction, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        let (root, view) = (self.root.clone(), cx.entity().downgrade());
        self.busy = Some(label);
        self.note = None;
        cx.notify();
        crate::backend::run(
            cx,
            async move { git::sync(&root, direction).await },
            move |answer, cx| {
                view.update(cx, |this, cx| {
                    this.busy = None;
                    this.note = Some(match answer {
                        Ok(text) => (format!("{label}: {text}"), false),
                        Err(e) => (e, true),
                    });
                    this.reload(cx);
                })
                .ok();
            },
        );
    }

    fn open_dialog(
        &mut self,
        kind: Kind,
        base: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.entity().downgrade();
        Compose::open(
            kind,
            self.root.clone(),
            &base,
            window,
            cx,
            move |text, cx| {
                view.update(cx, |this, cx| {
                    this.note = Some((text.to_owned(), false));
                    this.reload(cx);
                })
                .ok();
            },
        );
    }

    // ── Drawing ────────────────────────────────────────────────────────────────────────────────────────────

    fn pr_chip(pr: &BranchPr) -> impl IntoElement {
        let hex = match (pr.state, pr.draft) {
            (PrState::Merged, _) => ui::MERGED,
            (PrState::Closed, _) => theme::DANGER,
            (_, true) => theme::FAINT,
            _ => theme::OK,
        };
        let (mark, mark_hex) = match pr.checks {
            Checks::Failing => ("✕", theme::DANGER),
            Checks::Running => ("…", theme::WARN),
            Checks::Passing => ("✓", theme::OK),
            Checks::None => ("", theme::OK),
        };
        let url = pr.url.clone();
        div()
            .id("pr-chip")
            .h(px(20.0))
            .px(px(8.0))
            .flex()
            .items_center()
            .gap(px(4.0))
            .rounded(px(4.0))
            .border_1()
            .border_color(color(theme::BORDER))
            .text_size(px(theme::TEXT_XS))
            .cursor_pointer()
            .hover(|style| style.bg(color(theme::SURFACE)))
            .on_click(move |_e, _w, cx| cx.open_url(&url))
            .child(ui::dot(hex))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("#{}", pr.number)),
            )
            .children((!mark.is_empty()).then(|| {
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(color(mark_hex))
                    .child(mark)
            }))
    }

    /// The everyday moves for this repository: what each would do is in its label.
    fn quick_actions(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(s) = &self.summary else { return div() };
        let changed = s.changed();
        let on_default = s.default_branch.as_deref() == Some(s.branch.as_str());
        let open_pr = self
            .github
            .pr
            .as_ref()
            .is_some_and(|pr| pr.state == PrState::Open);
        let sync = match (s.upstream.is_some(), s.behind > 0, s.ahead > 0) {
            (true, true, true) => Some(("Sync", Direction::Both)),
            (true, true, false) => Some(("Pull", Direction::Pull)),
            (true, false, true) => Some(("Push", Direction::Push)),
            _ => None,
        };
        let failing = self
            .github
            .pr
            .as_ref()
            .filter(|pr| pr.checks == Checks::Failing);
        let note = self.note.clone();
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .children(note.map(|(text, bad)| div().max_w(px(360.0)).truncate().text_size(px(theme::TEXT_XS)).text_color(color(if bad { theme::DANGER } else { theme::MUTED })).child(text)))
            .children((changed > 0).then(|| {
                Button::new("git-commit").primary().xsmall().label(format!("Commit {changed}")).tooltip("Commit the changes, with a message written from the diff").on_click(cx.listener(|this, _e, window, cx| this.open_dialog(Kind::Commit, String::new(), window, cx)))
            }))
            .children(sync.map(|(label, direction)| {
                let text = format!("{label}{}{}", if s.ahead > 0 { format!(" ↑{}", s.ahead) } else { String::new() }, if s.behind > 0 { format!(" ↓{}", s.behind) } else { String::new() });
                Button::new("git-sync")
                    .outline()
                    .xsmall()
                    .label(if self.busy == Some(label) { "…".to_owned() } else { text })
                    .tooltip(format!("{} to pull, {} to push", s.behind, s.ahead))
                    .on_click(cx.listener(move |this, _e, _w, cx| this.sync(label, direction, cx)))
            }))
            .children((self.github.repo.is_some() && !on_default && !open_pr && !s.branch.is_empty()).then(|| {
                let (branch, base, needs_push) = (s.branch.clone(), s.default_branch.clone().unwrap_or_else(|| "main".to_owned()), s.upstream.is_none() || s.ahead > 0);
                Button::new("git-pr")
                    .outline()
                    .xsmall()
                    .label("Create PR")
                    .tooltip(if s.upstream.is_some() { "Open a pull request for this branch" } else { "Pushes the branch, then opens a pull request" })
                    .on_click(cx.listener(move |this, _e, window, cx| this.open_dialog(Kind::PullRequest { branch: branch.clone(), needs_push }, base.clone(), window, cx)))
            }))
            .children(failing.map(|pr| {
                let ask = format!(
                    "The checks on pull request #{} are failing: {}. Read the failing runs' logs with the github tool (actions run / checks), find the cause, fix it on this branch, commit, push, and tell me when the checks pass.",
                    pr.number,
                    pr.failing.join(", ")
                );
                Button::new("git-fix-checks").outline().danger().xsmall().label("Fix checks with the agent").tooltip(format!("Failing: {}", pr.failing.join(", "))).on_click(cx.listener(move |_this, _e, _w, cx| cx.emit(GitEvent::AskAgent(ask.clone()))))
            }))
    }

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let folder = std::path::Path::new(&self.root)
            .file_name()
            .map_or_else(|| self.root.clone(), |n| n.to_string_lossy().into_owned());
        let locals = self.branches.iter().filter(|b| !b.is_remote).count();
        let github = self
            .remote
            .as_ref()
            .and_then(|r| r.github_repo.clone().zip(r.github_url()));
        div()
            .h(px(40.0))
            .flex_none()
            .px(px(12.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(
                Button::new("git-back")
                    .ghost()
                    .xsmall()
                    .label("←")
                    .tooltip("Back to chat")
                    .on_click(cx.listener(|_this, _e, _w, cx| cx.emit(GitEvent::Back))),
            )
            .child(
                div()
                    .text_size(px(14.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .truncate()
                    .child(folder),
            )
            .children(self.summary.as_ref().map(|s| {
                mono(if s.branch.is_empty() {
                    "detached".to_owned()
                } else {
                    s.branch.clone()
                })
                .text_color(color(theme::MUTED))
            }))
            .child(faint(format!(
                "{locals} branch{}",
                if locals == 1 { "" } else { "es" }
            )))
            .children(self.github.pr.as_ref().map(Self::pr_chip))
            .child(div().flex_1())
            .child(self.quick_actions(cx))
            .children(match (github, &self.remote) {
                (Some((name, url)), _) => Some(
                    Button::new("git-github")
                        .ghost()
                        .xsmall()
                        .label(name)
                        .on_click(move |_e, _w, cx| cx.open_url(&url))
                        .into_any_element(),
                ),
                (None, Some(remote)) => Some(
                    div()
                        .max_w(px(320.0))
                        .truncate()
                        .text_size(px(theme::TEXT_XS))
                        .text_color(color(theme::MUTED))
                        .child(remote.url.clone())
                        .into_any_element(),
                ),
                _ => None,
            })
            .child(
                Button::new("git-refresh")
                    .ghost()
                    .xsmall()
                    .label("Refresh")
                    .loading(self.loading)
                    .on_click(cx.listener(|this, _e, _w, cx| this.reload(cx))),
            )
    }

    /// What waits on each tab: changes to commit, open pull requests and issues, workflows that failed.
    fn count(&self, tab: Tab) -> u32 {
        match tab {
            Tab::Changes => self.summary.as_ref().map_or(0, Summary::changed),
            Tab::PullRequests => self.github.open_prs.unwrap_or(0),
            Tab::Issues => self.github.open_issues.unwrap_or(0),
            Tab::Actions => self.github.failing_runs.unwrap_or(0),
            Tab::Releases | Tab::History => 0,
        }
    }

    fn tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .h(px(32.0))
            .flex_none()
            .px(px(8.0))
            .flex()
            .gap(px(4.0))
            .border_b_1()
            .border_color(color(theme::HAIRLINE))
            .children(TABS.iter().enumerate().map(|(i, (tab, label))| {
                let (tab, selected, count) = (*tab, self.tab == *tab, self.count(*tab));
                div()
                    .id(("git-tab", i))
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .cursor_pointer()
                    .text_size(px(theme::TEXT_SM))
                    .font_weight(if selected {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::MEDIUM
                    })
                    .text_color(color(if selected { theme::TEXT } else { theme::MUTED }))
                    .border_b_2()
                    .border_color(color(if selected {
                        theme::ACCENT
                    } else {
                        theme::PANEL
                    }))
                    .hover(|style| style.text_color(color(theme::TEXT)))
                    .on_click(cx.listener(move |this, _e, _w, cx| this.set_tab(tab, cx)))
                    .child(*label)
                    .children((count > 0).then(|| {
                        let red = tab == Tab::Actions;
                        div()
                            .min_w(px(16.0))
                            .h(px(16.0))
                            .px(px(4.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.0))
                            .bg(color(if red { theme::DANGER } else { theme::RAISED }))
                            .text_size(px(theme::TEXT_XS))
                            .text_color(color(if red { theme::ON_ACCENT } else { theme::MUTED }))
                            .child(count.to_string())
                    }))
            }))
    }

    fn history(&self) -> AnyElement {
        if self.commits.is_empty() {
            return div()
                .p(px(16.0))
                .child(muted(if self.loading {
                    "Loading…"
                } else {
                    "No commits, or not a git repository."
                }))
                .into_any_element();
        }
        let now = time::now();
        div()
            .id("git-history")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .py(px(4.0))
            .children(self.commits.iter().enumerate().map(|(i, c)| {
                div()
                    .h(px(28.0))
                    .px(px(16.0))
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .hover(|style| style.bg(color(theme::SURFACE)))
                    .child(ui::dot(if i == 0 { theme::WARN } else { theme::FAINT }))
                    .child(
                        div()
                            .w(px(56.0))
                            .flex_none()
                            .font_family(theme::MONO)
                            .text_size(px(theme::TEXT_XS))
                            .text_color(color(theme::MUTED))
                            .child(c.hash.chars().take(7).collect::<String>()),
                    )
                    .children((i == 0).then(|| {
                        div()
                            .px(px(4.0))
                            .rounded(px(4.0))
                            .border_1()
                            .border_color(color(theme::WARN))
                            .text_size(px(theme::TEXT_XS))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(color(theme::WARN))
                            .child("HEAD")
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(13.0))
                            .child(c.message.clone()),
                    )
                    .child(
                        div()
                            .max_w(px(160.0))
                            .flex_none()
                            .truncate()
                            .text_size(px(theme::TEXT_XS))
                            .text_color(color(theme::MUTED))
                            .child(c.author.clone()),
                    )
                    .child(
                        div()
                            .w(px(64.0))
                            .flex_none()
                            .flex()
                            .justify_end()
                            .text_size(px(theme::TEXT_XS))
                            .text_color(color(theme::MUTED))
                            .child(time::when(&c.date, now)),
                    )
            }))
            .into_any_element()
    }
}

impl Render for GitView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = if self.summary.is_none() && !self.loading {
            div()
                .p(px(16.0))
                .child(muted("This folder isn't a git repository."))
                .into_any_element()
        } else if self.summary.is_none() {
            div()
                .p(px(16.0))
                .child(Spinner::new().small())
                .into_any_element()
        } else {
            self.panel(window, cx)
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(color(theme::PANEL))
            .overflow_hidden()
            .child(self.header(cx))
            .child(self.tab_bar(cx))
            .child(div().flex_1().min_h_0().flex().flex_col().child(body))
    }
}
