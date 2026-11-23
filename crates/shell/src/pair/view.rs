//! The Pair page: working on the open project with a partner. Before it is paired, how to pair it; after, the
//! connection, the talk between the two people, and what each side's agent has been doing.

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::base::input::TextareaState;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, ClipboardItem, Context, Entity, EventEmitter, FontWeight, SharedString,
    Subscription, Window, div, px,
};
use vivianne_multiplayer::collab::{self, Store};
use vivianne_multiplayer::{ChatEntry, Event, Multiplayer, Status};
use vivianne_runtime::projects::{self, Project};

use crate::theme::{self, color};

/// What the page asks the app to do.
#[derive(Debug, Clone)]
pub enum PairEvent {
    Back,
}

pub struct PairView {
    folder: PathBuf,
    project: Option<Project>,
    status: Status,
    chat: Vec<ChatEntry>,
    store: Store,
    /// What is wrong with the last thing tried, in words.
    problem: Option<String>,
    // Pairing a project.
    name: Entity<InputState>,
    code: Entity<InputState>,
    remote: Entity<InputState>,
    // Talking to the partner.
    say: Entity<InputState>,
    _say_events: Subscription,
    _unused: Option<Entity<TextareaState>>,
}

impl EventEmitter<PairEvent> for PairView {}

fn mp() -> Option<Arc<Multiplayer>> {
    vivianne_multiplayer::global()
}

impl PairView {
    pub fn new(folder: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Your name, as your partner sees it")
        });
        let code =
            cx.new(|cx| InputState::new(window, cx).placeholder("Your partner's pairing code"));
        let remote =
            cx.new(|cx| InputState::new(window, cx).placeholder("Shared git remote (optional)"));
        let say = cx.new(|cx| InputState::new(window, cx).placeholder("Say something"));
        let say_events = cx.subscribe_in(&say, window, |view, _input, event, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                view.send_chat(window, cx);
            }
        });
        let mut view = Self {
            project: projects::paired_at(&folder),
            folder,
            status: Status::Disconnected,
            chat: Vec::new(),
            store: Store::default(),
            problem: None,
            name,
            code,
            remote,
            say,
            _say_events: say_events,
            _unused: None,
        };
        view.reload(cx);
        view.listen(cx);
        view
    }

    /// Reads what is shown from where it is kept.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.project = projects::paired_at(&self.folder);
        self.store = self
            .project
            .as_ref()
            .map(|p| collab::read(&p.id))
            .unwrap_or_default();
        let view = cx.entity().downgrade();
        crate::backend::run(
            cx,
            async {
                match mp() {
                    Some(mp) => Some((mp.status().await, mp.chat_log().await)),
                    None => None,
                }
            },
            move |state, cx| {
                view.update(cx, |view, cx| {
                    if let Some((status, chat)) = state {
                        (view.status, view.chat) = (status, chat);
                    }
                    cx.notify();
                })
                .ok();
            },
        );
    }

    /// Hears the connection's events for as long as the page lives. The connection opens in the background at
    /// start, so this waits for it if it is not there yet.
    fn listen(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                let Some(mp) = mp() else {
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs(2))
                        .await;
                    if this.update(cx, |_, _| ()).is_err() {
                        return;
                    }
                    continue;
                };
                let mut events = mp.subscribe();
                if this.update(cx, |view, cx| view.reload(cx)).is_err() {
                    return;
                }
                // The events arrive on the service runtime; each is carried over to the window by a channel.
                let (tx, mut rx) = futures::channel::mpsc::unbounded();
                crate::backend::spawn(async move {
                    while let Ok(event) = events.recv().await {
                        if tx.unbounded_send(event).is_err() {
                            break;
                        }
                    }
                });
                use futures::StreamExt as _;
                while let Some(event) = rx.next().await {
                    let updated = this.update(cx, |view, cx| {
                        match event {
                            Event::Status(status) => view.status = status,
                            Event::Chat(entry) => view.chat.push(entry),
                            Event::Collab(_) => {
                                view.store = view
                                    .project
                                    .as_ref()
                                    .map(|p| collab::read(&p.id))
                                    .unwrap_or_default();
                            }
                        }
                        cx.notify();
                    });
                    if updated.is_err() {
                        return;
                    }
                }
                return;
            }
        })
        .detach();
    }

    fn text(input: &Entity<InputState>, cx: &Context<Self>) -> String {
        input.read(cx).value().trim().to_owned()
    }

    /// Pairs the open project with the partner whose code was typed, then connects.
    fn pair(&mut self, cx: &mut Context<Self>) {
        let (code, remote, name) = (
            Self::text(&self.code, cx),
            Self::text(&self.remote, cx),
            Self::text(&self.name, cx),
        );
        match projects::pair(&self.folder, &code, None, Some(&remote)) {
            Ok(_) => {
                self.problem = None;
                self.reload(cx);
                self.connect(Some(name), cx);
            }
            Err(why) => self.problem = Some(why),
        }
        cx.notify();
    }

    fn connect(&mut self, name: Option<String>, cx: &mut Context<Self>) {
        let Some(code) = self
            .project
            .as_ref()
            .and_then(Project::pairing)
            .map(|p| p.peer_id)
            .or_else(|| Some(Self::text(&self.code, cx)))
            .filter(|c| !c.is_empty())
        else {
            return;
        };
        let view = cx.entity().downgrade();
        crate::backend::run(
            cx,
            async move {
                let mp = mp().ok_or(
                    "Multiplayer isn't running: it needs a network connection.".to_owned(),
                )?;
                if let Some(name) = name.filter(|n| !n.is_empty()) {
                    mp.set_display_name(Some(name)).await;
                }
                mp.connect(&code).await
            },
            move |result: Result<(), String>, cx| {
                view.update(cx, |view, cx| {
                    view.problem = result.err();
                    cx.notify();
                })
                .ok();
            },
        );
    }

    fn disconnect(cx: &mut Context<Self>) {
        crate::backend::run(
            cx,
            async {
                if let Some(mp) = mp() {
                    mp.disconnect().await
                }
            },
            |(), _cx| (),
        );
    }

    fn send_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = Self::text(&self.say, cx);
        if text.is_empty() {
            return;
        }
        self.say
            .update(cx, |input, cx| input.set_value("", window, cx));
        let view = cx.entity().downgrade();
        crate::backend::run(
            cx,
            async move {
                mp().ok_or("Not connected.".to_owned())?
                    .send_chat(text)
                    .await
            },
            move |result: Result<(), String>, cx| {
                view.update(cx, |view, cx| {
                    view.problem = result.err();
                    cx.notify();
                })
                .ok();
            },
        );
    }

    fn dismiss_notes(&mut self, cx: &mut Context<Self>) {
        if let Some(project) = &self.project {
            collab::with_store(&project.id, |s| s.notes.clear());
            self.store = collab::read(&project.id);
        }
        cx.notify();
    }

    // ── Drawing ───────────────────────────────────────────────────────────────────────────────────────────────

    fn heading(text: impl Into<SharedString>) -> gpui_kit::Div {
        div()
            .text_size(px(theme::TEXT_SM))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(color(theme::MUTED))
            .child(text.into().to_uppercase())
    }

    fn muted(text: impl Into<SharedString>) -> gpui_kit::Div {
        div()
            .text_size(px(theme::TEXT_SM))
            .text_color(color(theme::MUTED))
            .child(text.into())
    }

    fn status_line(&self) -> (String, u32) {
        match &self.status {
            Status::Connected {
                peer_display_name, ..
            } => (
                format!(
                    "Connected to {}",
                    peer_display_name.as_deref().unwrap_or("your partner")
                ),
                theme::OK,
            ),
            Status::Connecting => ("Connecting…".to_owned(), theme::WARN),
            Status::Failed { reason } => (reason.clone(), theme::DANGER),
            Status::Disconnected => ("Not connected".to_owned(), theme::FAINT),
        }
    }

    fn setup(&self, cx: &mut Context<Self>) -> AnyElement {
        let id = mp().map(|mp| mp.my_id.clone());
        div()
            .flex()
            .flex_col()
            .gap(px(14.0))
            .max_w(px(520.0))
            .child(div().text_size(px(theme::TEXT_LG)).font_weight(FontWeight::SEMIBOLD).child("Work on this project with a partner"))
            .child(Self::muted("Two people, two machines, one project, each with their own agent. Each agent is told what the other did, the code is kept in step, and you can talk here. Nothing goes through a server."))
            .child(Self::heading("Your pairing code"))
            .child(match id {
                Some(id) => {
                    let copy = id.clone();
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(div().flex_1().min_w_0().truncate().font_family(theme::MONO).text_size(px(theme::TEXT_SM)).child(id))
                        .child(Button::new("copy-code").outline().xsmall().label("Copy").on_click(move |_e, _w, cx| cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))))
                        .into_any_element()
                }
                None => Self::muted("Starting… (multiplayer needs a network connection)").into_any_element(),
            })
            .child(Self::muted("Give this to your partner; they give you theirs."))
            .child(Self::heading("Pair this project"))
            .child(Input::new(&self.name))
            .child(Input::new(&self.code))
            .child(Input::new(&self.remote))
            .children(self.problem.clone().map(|p| div().text_size(px(theme::TEXT_SM)).text_color(color(theme::DANGER)).child(p)))
            .child(div().child(Button::new("pair-project").primary().label("Pair and connect").on_click(cx.listener(|view, _e, _w, cx| view.pair(cx)))))
            .into_any_element()
    }

    fn partner(&self) -> String {
        self.project
            .as_ref()
            .and_then(Project::pairing)
            .and_then(|p| p.peer_display_name)
            .unwrap_or_else(|| "your partner".to_owned())
    }

    fn paired(&self, cx: &mut Context<Self>) -> AnyElement {
        let (line, dot) = self.status_line();
        let connected = matches!(self.status, Status::Connected { .. });
        let header = div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .child(div().size(px(8.0)).rounded_full().bg(color(dot)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.0))
                    .child(line),
            )
            .child(if connected {
                Button::new("pair-disconnect")
                    .outline()
                    .xsmall()
                    .label("Disconnect")
                    .on_click(cx.listener(|_view, _e, _w, cx| Self::disconnect(cx)))
                    .into_any_element()
            } else {
                Button::new("pair-connect")
                    .primary()
                    .xsmall()
                    .label("Connect")
                    .on_click(cx.listener(|view, _e, _w, cx| view.connect(None, cx)))
                    .into_any_element()
            });
        let notes: Vec<AnyElement> = self
            .store
            .notes
            .iter()
            .map(|n| {
                div()
                    .text_size(px(theme::TEXT_SM))
                    .text_color(color(theme::WARN))
                    .child(format!("⚠ {}", n.text))
                    .into_any_element()
            })
            .collect();
        let notes = (!notes.is_empty()).then(|| {
            div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .p(px(10.0))
                .rounded(px(theme::RADIUS_CONTROL))
                .bg(color(theme::SURFACE))
                .children(notes)
                .child(
                    div().child(
                        Button::new("dismiss-notes")
                            .ghost()
                            .xsmall()
                            .label("Dismiss")
                            .on_click(cx.listener(|view, _e, _w, cx| view.dismiss_notes(cx))),
                    ),
                )
        });
        let chat = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(Self::heading("Talk"))
            .child(
                div()
                    .id("pair-chat")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .children(self.chat.iter().map(|e| {
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(theme::TEXT_XS))
                                    .text_color(color(theme::FAINT))
                                    .child(if e.from_peer {
                                        self.partner()
                                    } else {
                                        "You".to_owned()
                                    }),
                            )
                            .child(div().text_size(px(13.0)).child(e.text.clone()))
                    })),
            )
            .child(Input::new(&self.say));
        let me = |e: &collab::ActivityEntry| (e.at, collab::entry_line(e), "You");
        let them = |e: &collab::ActivityEntry| (e.at, collab::entry_line(e), "Partner");
        let mut feed: Vec<(u64, String, &str)> = self
            .store
            .my_activity
            .iter()
            .map(me)
            .chain(self.store.peer_activity.iter().map(them))
            .collect();
        feed.sort_by_key(|f| std::cmp::Reverse(f.0));
        let activity = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(Self::heading("What the agents did"))
            .child(
                div()
                    .id("pair-activity")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .children(if feed.is_empty() {
                        vec![
                            Self::muted(
                                "Nothing yet. Each finished run shows up here, for both of you.",
                            )
                            .into_any_element(),
                        ]
                    } else {
                        feed.into_iter()
                            .take(40)
                            .map(|(_, line, who)| {
                                div()
                                    .text_size(px(theme::TEXT_SM))
                                    .child(format!("{who}{}", line.trim_start_matches('-')))
                                    .into_any_element()
                            })
                            .collect()
                    }),
            )
            .children(self.store.last_sync.clone().map(Self::muted))
            .child(Self::muted(self.usage_line()));
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(header)
            .children(self.problem.clone().map(|p| {
                div()
                    .text_size(px(theme::TEXT_SM))
                    .text_color(color(theme::DANGER))
                    .child(p)
            }))
            .children(notes)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .gap(px(24.0))
                    .child(chat)
                    .child(activity),
            )
            .into_any_element()
    }

    /// What each side has spent on the project, in dollars, over the days that are known.
    fn usage_line(&self) -> String {
        let theirs: f64 = self.store.peer_usage.values().map(|d| d.cost).sum();
        if self.store.peer_usage.is_empty() {
            "Partner's usage: not reported yet".to_owned()
        } else {
            format!("Partner's usage on this project: ${theirs:.2}")
        }
    }
}

impl Render for PairView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = if self.project.is_some() {
            self.paired(cx)
        } else {
            self.setup(cx)
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(color(theme::PANEL))
            .overflow_hidden()
            .child(
                div()
                    .h(px(40.0))
                    .flex_none()
                    .px(px(12.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        Button::new("pair-back")
                            .ghost()
                            .xsmall()
                            .label("←")
                            .tooltip("Back")
                            .on_click(cx.listener(|_view, _e, _w, cx| cx.emit(PairEvent::Back))),
                    )
                    .child(
                        div()
                            .text_size(px(14.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Pair"),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .px(px(20.0))
                    .pb(px(16.0))
                    .child(body),
            )
    }
}
