//! The chat screen: transcript (or the empty state), composer, status line.

mod composer;
mod items;
mod models;
mod own_tools;
mod session;
mod thread_usage;
mod toolcalls;
mod transcript;

use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::combobox::{ComboboxEvent, ComboboxState};
use gpui_kit::component::input::InputEvent;
use gpui_kit::component::message_scroller::{MessageScroller, MessageScrollerState};
use gpui_kit::component::searchable_list::{SearchableListItem as _, SearchableVec};
use tokio::runtime::Runtime;
use vivianne_providers::env::Selection;
use gpui_kit::prelude::*;
use gpui_kit::{
    App, Context, Entity, FocusHandle, Focusable, FontWeight, PathPromptOptions, StyleRefinement,
    Subscription, Window, div, px,
};

use crate::theme::{self, color};
use composer::{Composer, Mode, Thinking, compose_message, display_message};
use items::{Change, Item, apply};
use models::{ModelChoice, Picker, choices};
use session::{Finished, Run, Session};

pub use session::Session as ChatSession;

/// What a chat needs from the app around it to find models and switch between them.
pub struct ChatHost {
    pub runtime: Arc<Runtime>,
    pub workspace: PathBuf,
}

const PLACEHOLDER: &str = "Message Vivianne…";
/// Agent events that arrive faster than this are applied together, so a fast stream redraws the
/// transcript a few dozen times a second rather than once per token.
const EVENT_BATCH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(40);

pub struct ChatView {
    items: Vec<Item>,
    composer: Composer,
    /// Which rows of the transcript are on screen; only those are laid out and drawn.
    scroller: Entity<MessageScrollerState>,
    /// `Err` holds what is wrong with the setup, shown as the composer hint.
    session: Result<Session, String>,
    /// The provider and model the next message goes to.
    selection: Selection,
    host: ChatHost,
    /// Every model the saved keys reach; empty until they are looked up.
    models: Vec<ModelChoice>,
    picker: Picker,
    /// Name of the project folder, shown on the empty screen.
    project: String,
    running: bool,
    /// Tool calls the user has folded shut, by id.
    folded_tools: HashSet<String>,
    /// What this thread has used, shown in the status row. Read again after each reply.
    thread_usage: Option<vivianne_ai::inference::usage_store::DayUsage>,
    /// Where the context window goes, worked out when the history or the mode changes (not on every redraw).
    context: Option<vivianne_runtime::context::Breakdown>,
    /// The thread's name and who gave it (`user` or `auto`); `None` until something has been said.
    title: Option<(String, &'static str)>,
    /// What the newest run changed in the project, for the side panel and for undoing it.
    changes: Option<vivianne_runtime::checkpoints::LastRun>,
    /// What the person picks on a notification arrives here, from whatever thread the system uses.
    toasts: futures::channel::mpsc::UnboundedSender<(String, crate::notify::Choice)>,
    _input_events: Subscription,
    _picker_events: Subscription,
}

/// Starting points shown before the first message. Picking one fills the composer.
const SUGGESTIONS: [&str; 3] = [
    "Summarise this project",
    "Find and fix a bug",
    "Write tests for a module",
];

impl ChatView {
    pub fn new(
        session: Result<Session, String>,
        selection: Selection,
        project: String,
        host: ChatHost,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let hint = match &session {
            Ok(_) => PLACEHOLDER.to_owned(),
            Err(problem) => problem.clone(),
        };
        let composer = Composer::new(hint, window, cx);
        let input_events = cx.subscribe_in(
            &composer.input,
            window,
            |chat, _input, event, window, cx| {
                match event {
                    InputEvent::PressEnter { .. } => chat.send(window, cx),
                    // The send button's enabled state follows the text.
                    InputEvent::Change => cx.notify(),
                    _ => {}
                }
            },
        );
        let picker: Picker =
            cx.new(|cx| ComboboxState::new(SearchableVec::new(Vec::<ModelChoice>::new()), vec![], window, cx).searchable(true));
        let picker_events = cx.subscribe_in(&picker, window, |chat, _picker, event, window, cx| {
            if let ComboboxEvent::Change(values) = event
                && let Some(value) = values.first()
            {
                chat.select_model(value, window, cx);
            }
        });
        Self::look_up_models(&host, window, cx);
        let (toasts, mut toast_choices) = futures::channel::mpsc::unbounded::<(String, crate::notify::Choice)>();
        cx.spawn_in(window, async move |chat, cx| {
            use futures::StreamExt as _;
            while let Some((id, choice)) = toast_choices.next().await {
                if chat.update_in(cx, |chat, window, cx| chat.answer_from_notification(&id, choice, window, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        let context = session.as_ref().ok().and_then(Session::breakdown);
        Self {
            items: Vec::new(),
            composer,
            scroller: cx.new(|cx| MessageScrollerState::new(0, cx)),
            session,
            selection,
            host,
            models: Vec::new(),
            picker,
            project,
            running: false,
            folded_tools: HashSet::new(),
            thread_usage: None,
            context,
            title: None,
            changes: None,
            toasts,
            _input_events: input_events,
            _picker_events: picker_events,
        }
    }

    /// Ask every provider with a saved key which models it offers, without holding up the window.
    fn look_up_models(host: &ChatHost, window: &mut Window, cx: &mut Context<Self>) {
        let (found, models) = tokio::sync::oneshot::channel();
        host.runtime.spawn(async move {
            let (discovered, agents) = tokio::join!(vivianne_ai::host::discover_models(false), vivianne_providers::acp::harnesses());
            // Only agents whose launcher is here can run.
            let agents: Vec<(String, String)> = agents.into_iter().filter(|a| a.available).map(|a| (a.id, a.name)).collect();
            let _ = found.send((discovered, agents));
        });
        cx.spawn_in(window, async move |chat, cx| {
            let Ok((discovered, agents)) = models.await else { return };
            chat.update_in(cx, |chat, window, cx| chat.show_models(&discovered, &agents, window, cx)).ok();
        })
        .detach();
    }

    fn show_models(
        &mut self,
        discovered: &[vivianne_ai::ai::model_discovery::DiscoveredModel],
        agents: &[(String, String)],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.models = choices(discovered, agents);
        self.refresh_context();
        let current = ModelChoice::value_of(&self.selection.provider, &self.selection.model);
        let items = SearchableVec::new(self.models.clone());
        self.picker.update(cx, |picker, cx| {
            picker.set_items(items, window, cx);
            picker.set_selected_values(&[current], window, cx);
        });
        cx.notify();
    }

    /// Puts the picker's tick back on the model the thread is really using, after a pick that did not switch:
    /// a tick left on the pick would make picking it again do nothing.
    fn show_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = ModelChoice::value_of(&self.selection.provider, &self.selection.model);
        self.picker.update(cx, |picker, cx| picker.set_selected_values(&[current], window, cx));
    }

    /// Switch the thread to the picked model; the conversation so far stays.
    fn select_model(&mut self, value: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(choice) = self.models.iter().find(|m| m.value() == value).cloned() else {
            return;
        };
        if self.running {
            self.push_item(Item::Notice("Wait for the reply to finish before switching models.".to_owned()), cx);
            self.show_selection(window, cx);
            return;
        }
        if choice.is_agent() {
            self.use_coding_agent(&choice, window, cx);
            return;
        }
        match crate::agent_setup::agent_for(&choice.provider, &choice.id, &self.host.workspace) {
            Ok(agent) => self.use_agent(agent, &choice, cx),
            Err(problem) => {
                self.push_item(Item::Notice(format!("Can't use {}: {problem}", choice.name)), cx);
                self.show_selection(window, cx);
            }
        }
    }

    /// Hand the thread to a coding agent, once it is known to be signed in: an agent that isn't would answer
    /// the first message with an authentication error.
    fn use_coding_agent(&mut self, choice: &ModelChoice, window: &mut Window, cx: &mut Context<Self>) {
        self.push_item(Item::Notice(format!("Checking that {} is signed in…", choice.name)), cx);
        let id = choice.id.clone();
        let Some(answer) = crate::backend::spawn(async move { vivianne_providers::acp::check_auth(&id, vivianne_providers::acp::Depth::Quick).await }) else {
            self.push_item(Item::Notice("Coding agents aren't available: the app's services didn't start.".to_owned()), cx);
            self.show_selection(window, cx);
            return;
        };
        let choice = choice.clone();
        cx.spawn_in(window, async move |chat, cx| {
            let Ok(result) = answer.await else { return };
            chat.update_in(cx, |chat, window, cx| chat.after_sign_in_check(&choice, result, window, cx)).ok();
        })
        .detach();
    }

    /// Switches to the agent if it is signed in. Otherwise the picker goes back to the model in use and the
    /// agent is signed in from here: in a window with its own terminal, or the browser the agent opens.
    fn after_sign_in_check(
        &mut self,
        choice: &ModelChoice,
        result: Result<vivianne_providers::acp::AuthState, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use vivianne_providers::acp::{AuthKind, AuthState};
        match result {
            Ok(AuthState::SignedIn) => self.switch_to_coding_agent(choice, cx),
            Ok(AuthState::NeedsSignIn(methods)) => {
                self.show_selection(window, cx);
                let chat = cx.entity().downgrade();
                let picked = choice.clone();
                let switch: Rc<dyn Fn(&mut App)> = Rc::new(move |cx| {
                    let _ = chat.update(cx, |chat, cx| chat.switch_to_coding_agent(&picked, cx));
                });
                let terminal = methods.iter().find_map(|m| (m.kind == AuthKind::Terminal).then(|| m.command.clone()).flatten());
                let browser = methods.iter().find(|m| m.kind == AuthKind::Agent).map(|m| m.id.clone());
                if let Some(command) = terminal {
                    self.push_item(Item::Notice(format!("{} needs you to sign in. A window is opening to do it; this chat switches by itself once you have.", choice.name)), cx);
                    crate::signin::open(&choice.id, &choice.name, &command, switch, cx);
                } else if let Some(method) = browser {
                    self.push_item(Item::Notice(format!("{} needs you to sign in. Finish in the browser window that opens; this chat switches by itself.", choice.name)), cx);
                    self.sign_in_with_browser(choice, &method, window, cx);
                } else {
                    self.push_item(Item::Notice(format!("{} needs you to sign in, and offers no way Vivianne can run. Sign in to it once in a terminal.", choice.name)), cx);
                }
            }
            Ok(AuthState::Failed(why)) | Err(why) => {
                self.push_item(Item::Notice(format!("Can't use {}: {why} Set it up in Settings, Providers, Coding agents.", choice.name)), cx);
                self.show_selection(window, cx);
            }
        }
    }

    /// Runs the agent's own sign-in (which opens a browser) and switches to it when that worked.
    fn sign_in_with_browser(&mut self, choice: &ModelChoice, method: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (id, method) = (choice.id.clone(), method.to_owned());
        let Some(answer) = crate::backend::spawn(async move { vivianne_providers::acp::sign_in(&id, &method).await }) else { return };
        let choice = choice.clone();
        cx.spawn_in(window, async move |chat, cx| {
            let Ok(result) = answer.await else { return };
            crate::backend::note_sign_in(&choice.id, &result);
            chat.update_in(cx, |chat, window, cx| match result {
                Ok(vivianne_providers::acp::AuthState::SignedIn) => chat.switch_to_coding_agent(&choice, cx),
                Ok(vivianne_providers::acp::AuthState::NeedsSignIn(_)) => {
                    chat.push_item(Item::Notice(format!("{} still isn't signed in. Pick it again to retry.", choice.name)), cx);
                    chat.show_selection(window, cx);
                }
                Ok(vivianne_providers::acp::AuthState::Failed(why)) | Err(why) => {
                    chat.push_item(Item::Notice(format!("Signing in to {} failed: {why}", choice.name)), cx);
                    chat.show_selection(window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Hand the thread to a coding agent. The conversation so far stays in the transcript; the agent starts
    /// its own session the first time it is asked.
    fn switch_to_coding_agent(&mut self, choice: &ModelChoice, cx: &mut Context<Self>) {
        let Some(backend) = crate::backend::get() else {
            self.push_item(Item::Notice("Coding agents aren't available: the app's services didn't start.".to_owned()), cx);
            return;
        };
        let workspace = self.host.workspace.clone();
        match self.session.as_mut() {
            Ok(session) => session.use_harness(&choice.id, backend.acp.clone(), workspace),
            Err(_) => {
                let Ok(agent) = crate::agent_setup::placeholder_agent(&self.host.workspace) else { return };
                let mut session = Session::new(agent, self.host.runtime.clone(), self.host.workspace.clone());
                session.use_harness(&choice.id, backend.acp.clone(), workspace);
                self.session = Ok(session);
            }
        }
        self.selection = Selection { provider: choice.provider.clone(), model: choice.id.clone() };
        self.refresh_context();
        vivianne_telemetry::track("model.switch", serde_json::json!({ "provider": choice.provider, "model": choice.id }));
        self.push_item(Item::Notice(format!("{} will work in this project's folder with its own tools, and asks before it acts.", choice.name)), cx);
    }

    /// The user's answer to a coding agent's request.
    fn answer_approval(&mut self, id: &str, allow: bool, cx: &mut Context<Self>) {
        // Answered here, so the notification asking the same thing is taken down.
        crate::notify::withdraw(id);
        if let Ok(session) = &self.session {
            session.answer(id, allow);
        }
        if let Some(index) = items::answer_approval(&mut self.items, id, allow) {
            self.scroller.update(cx, |scroller, cx| scroller.remeasure_items(index..index + 1, cx));
        }
        cx.notify();
    }

    fn use_agent(&mut self, agent: vivianne_runtime::Agent, choice: &ModelChoice, cx: &mut Context<Self>) {
        match self.session.as_mut() {
            Ok(session) => session.switch_agent(agent, choice.context_window),
            Err(_) => self.session = Ok(Session::new(agent, self.host.runtime.clone(), self.host.workspace.clone())),
        }
        self.selection = Selection { provider: choice.provider.clone(), model: choice.id.clone() };
        vivianne_telemetry::track("model.switch", serde_json::json!({ "provider": choice.provider, "model": choice.id }));
        if let Err(problem) = vivianne_ai::host::remember_last_model(&choice.provider, &choice.id) {
            self.push_item(Item::Notice(format!("Switched, but couldn't remember the choice: {problem}")), cx);
        }
        cx.notify();
    }

    /// Add an item at the end of the transcript.
    fn push_item(&mut self, item: Item, cx: &mut Context<Self>) {
        self.items.push(item);
        self.scroller.update(cx, |scroller, cx| scroller.append(1, cx));
        cx.notify();
    }

    /// Fold a batch of agent events in, telling the list only which rows changed.
    fn apply_events(&mut self, events: &[vivianne_runtime::AgentEvent], cx: &mut Context<Self>) {
        for event in events {
            // Something waiting on you while the window is not in front: say so where you will see it.
            if let vivianne_runtime::AgentEvent::Approval { id, title } = event
                && cx.active_window().is_none()
            {
                let toasts = self.toasts.clone();
                let tag = id.clone();
                crate::notify::show(id, "Vivianne needs your approval", title, crate::notify::Quick::ApproveReject, move |choice| {
                    let _ = toasts.unbounded_send((tag.clone(), choice));
                });
            }
            match apply(&mut self.items, event) {
                Change::Nothing => {}
                Change::Appended => {
                    self.scroller.update(cx, |scroller, cx| scroller.append(1, cx));
                }
                Change::Updated(index) => {
                    self.scroller.update(cx, |scroller, cx| scroller.remeasure_items(index..index + 1, cx));
                }
            }
        }
        cx.notify();
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        let Ok(session) = self.session.as_mut() else {
            return;
        };
        let text = self.composer.input.read(cx).value().to_string();
        let files = &self.composer.attachments;
        if text.trim().is_empty() && files.is_empty() {
            return;
        }
        let Some(run) = session.send(compose_message(&text, files), self.composer.thinking) else {
            return;
        };
        let shown = display_message(&text, files);
        if self.title.is_none() {
            self.ask_for_title(&text, cx);
        }
        self.push_item(Item::User(shown), cx);
        self.composer.attachments.clear();
        self.composer
            .input
            .update(cx, |input, cx| input.clean(window, cx));
        self.running = true;
        let Run { mut events, done } = run;
        cx.spawn(async move |this, cx| {
            while let Some(first) = events.recv().await {
                let mut batch = vec![first];
                while let Ok(more) = events.try_recv() {
                    batch.push(more);
                }
                if this.update(cx, |chat, cx| chat.apply_events(&batch, cx)).is_err() {
                    return;
                }
                cx.background_executor().timer(EVENT_BATCH_INTERVAL).await;
            }
            // No answer means the run's task died; the chat must not stay "working" for ever.
            let finished = done.await.ok();
            this.update(cx, |chat, cx| chat.finish(finished, cx)).ok();
        })
        .detach();
        cx.notify();
    }

    /// What the person chose on a notification.
    fn answer_from_notification(&mut self, id: &str, choice: crate::notify::Choice, window: &mut Window, cx: &mut Context<Self>) {
        use crate::notify::Choice;
        match choice {
            Choice::Approve => self.answer_approval(id, true, cx),
            Choice::Reject => self.answer_approval(id, false, cx),
            Choice::Reply(text) => {
                self.composer.input.update(cx, |input, cx| input.set_value(text, window, cx));
                self.send(window, cx);
                window.activate_window();
            }
            Choice::Open => window.activate_window(),
        }
    }

    /// A finished reply, said on the desktop when the window is not in front and the person asked for it
    /// (`notifications.enabled` in settings.json, as in the old app; off by default).
    fn notify_finished(&self, cx: &mut Context<Self>) {
        let Ok(session) = &self.session else { return };
        let Some(history) = session.history() else { return };
        let body = crate::notify::last_reply(history);
        let toasts = self.toasts.clone();
        crate::backend::run(cx, async { vivianne_ai::host::read_settings_json().await }, move |settings, _cx| {
            if settings["notifications"]["enabled"].as_bool() == Some(true) {
                let tag = uuid::Uuid::new_v4().to_string();
                let reply_tag = tag.clone();
                crate::notify::show(&tag, "Vivianne", &body, crate::notify::Quick::Reply, move |choice| {
                    let _ = toasts.unbounded_send((reply_tag.clone(), choice));
                });
            }
        });
    }

    fn finish(&mut self, finished: Option<Finished>, cx: &mut Context<Self>) {
        self.running = false;
        if let Ok(session) = self.session.as_mut()
            && let Some(error) = match finished {
                Some(finished) => session.finish(finished),
                None => Some(session.lost()),
            }
        {
            self.push_item(Item::Notice(format!("Error: {error}")), cx);
        }
        self.refresh_thread_usage(cx);
        self.refresh_context();
        self.refresh_changes(cx);
        self.save();
        if cx.active_window().is_none() {
            self.notify_finished(cx);
        }
        cx.notify();
    }

    /// What the newest run changed, read from the checkpoints taken around it.
    #[must_use]
    pub fn changes(&self) -> Option<&vivianne_runtime::checkpoints::LastRun> {
        self.changes.as_ref()
    }

    fn refresh_changes(&mut self, cx: &mut Context<Self>) {
        let Ok(session) = &self.session else { return };
        let (folder, thread) = (session.folder().to_path_buf(), session.thread_id().to_owned());
        let chat = cx.entity().downgrade();
        crate::backend::run(cx, async move { vivianne_runtime::checkpoints::last_run(&folder, &thread).await }, move |found, cx| {
            chat.update(cx, |chat, cx| {
                chat.changes = found.ok().flatten();
                cx.notify();
            })
            .ok();
        });
    }

    /// Puts the project back the way it was before the newest run. The files the run changed are restored and the
    /// ones it created are removed; a safety checkpoint is taken first, so this can itself be undone.
    pub fn undo_last_run(&mut self, cx: &mut Context<Self>) {
        let (Ok(session), Some(run)) = (&self.session, &self.changes) else { return };
        if self.running {
            self.push_item(Item::Notice("Wait for the reply to finish, or stop it, before undoing it.".to_owned()), cx);
            return;
        }
        let (folder, thread, before) = (session.folder().to_path_buf(), session.thread_id().to_owned(), run.before);
        let chat = cx.entity().downgrade();
        crate::backend::run(cx, async move { vivianne_runtime::checkpoints::rollback(&folder, &thread, before).await }, move |result, cx| {
            chat.update(cx, |chat, cx| {
                let notice = match result {
                    Ok(report) => format!("Undid the last run: restored {} files, removed {}.", report.restored.len(), report.removed.len()),
                    Err(problem) => format!("Couldn't undo the last run: {problem}"),
                };
                chat.push_item(Item::Notice(notice), cx);
                chat.refresh_changes(cx);
            })
            .ok();
        });
    }

    /// Works out where the context window goes. Kept as it was while a run owns the history.
    fn refresh_context(&mut self) {
        let window = self.window();
        if let Ok(session) = &self.session {
            session.set_context_window(window);
            if let Some(parts) = session.breakdown() {
                self.context = Some(parts);
            }
        }
    }

    /// How many tokens the chosen model holds, when its provider says.
    fn window(&self) -> Option<u32> {
        self.models.iter().find(|m| m.provider == self.selection.provider && m.id == self.selection.model).and_then(|m| m.context_window)
    }

    /// What the side panel shows about the window. `None` for a coding agent, which keeps its own conversation.
    #[must_use]
    pub fn context(&self) -> Option<crate::rail::ContextView> {
        let window = self.window();
        let own = self.selection.provider != models::AGENT_PROVIDER;
        self.context.filter(|_| own).map(|breakdown| crate::rail::ContextView { breakdown, window, mode: self.composer.mode })
    }

    /// Reads what the thread has used so far, for the status row. Everything the turn spent was recorded before
    /// the turn's events ended, so this sees it.
    fn refresh_thread_usage(&mut self, cx: &mut Context<Self>) {
        let Ok(session) = &self.session else { return };
        let thread = session.thread_id().to_owned();
        let chat = cx.entity().downgrade();
        crate::backend::run(cx, async move { vivianne_ai::inference::usage_store::thread_total(&thread).await }, move |total, cx| {
            chat.update(cx, |chat, cx| {
                chat.thread_usage = Some(total);
                cx.notify();
            })
            .ok();
        });
    }

    /// Stop the run in progress.
    fn cancel(&mut self, cx: &mut Context<Self>) {
        if !self.running {
            return;
        }
        if let Ok(session) = self.session.as_mut() {
            session.cancel();
        }
        self.running = false;
        self.push_item(Item::Notice("Stopped.".to_owned()), cx);
    }

    /// Open the system file picker and attach what is chosen.
    fn pick_files(cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = chosen.await {
                this.update(cx, |chat, cx| chat.attach_paths(&paths, cx))
                    .ok();
            }
        })
        .detach();
    }

    fn attach_paths(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        self.composer.attach(paths);
        cx.notify();
    }

    fn remove_attachment(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.composer.attachments.len() {
            self.composer.attachments.remove(index);
            cx.notify();
        }
    }

    /// Fold or unfold one tool call's output.
    fn toggle_tool(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.folded_tools.remove(id) {
            self.folded_tools.insert(id.to_owned());
        }
        cx.notify();
    }

    fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        self.composer.mode = mode;
        if let Ok(session) = self.session.as_mut() {
            session.set_mode(mode);
        }
        self.refresh_context();
        cx.notify();
    }

    fn set_thinking(&mut self, level: Thinking, cx: &mut Context<Self>) {
        self.composer.thinking = level;
        cx.notify();
    }

    /// Tab title: the first thing the user said, shortened.
    #[must_use]
    pub fn title(&self) -> String {
        if let Some((title, _)) = &self.title {
            return shorten(title, TITLE_MAX_CHARS);
        }
        let first = self.items.iter().find_map(|item| match item {
            Item::User(text) => Some(text.as_str()),
            _ => None,
        });
        match first {
            None => NEW_THREAD_TITLE.to_owned(),
            Some(text) => shorten(text, TITLE_MAX_CHARS),
        }
    }

    /// The thread's id: the name its usage and its saved file go by.
    #[must_use]
    pub fn thread_id(&self) -> Option<&str> {
        self.session.as_ref().ok().map(Session::thread_id)
    }

    /// Fills the thread from a saved one: what was said, its name and its mode.
    pub fn restore(&mut self, saved: vivianne_runtime::store::SavedThread, cx: &mut Context<Self>) {
        let Ok(session) = self.session.as_mut() else { return };
        self.items = items::from_history(&saved.history);
        session.restore(saved.id, saved.history);
        session.set_mode(saved.mode);
        self.composer.mode = saved.mode;
        self.title = Some((saved.title, if saved.title_source == vivianne_runtime::title::USER { vivianne_runtime::title::USER } else { vivianne_runtime::title::AUTO }));
        self.scroller.update(cx, |scroller, cx| scroller.append(self.items.len(), cx));
        self.refresh_context();
        cx.notify();
    }

    /// Writes the thread to disk so it survives closing the app. Nothing is written while a run owns the history
    /// or before anything has been said.
    fn save(&self) {
        let Ok(session) = &self.session else { return };
        let Some(history) = session.history().filter(|h| !h.is_empty()) else { return };
        let (title, source) = vivianne_runtime::title::decide(
            self.title.as_ref().map(|(t, s)| (t.as_str(), *s)),
            vivianne_runtime::title::first_ask(history),
        );
        let thread = vivianne_runtime::store::SavedThread {
            id: session.thread_id().to_owned(),
            workspace: self.host.workspace.display().to_string(),
            title,
            title_source: source.to_owned(),
            provider: self.selection.provider.clone(),
            model: self.selection.model.clone(),
            mode: self.composer.mode,
            updated_at: vivianne_runtime::store::now(),
            history: history.to_vec(),
        };
        // A failed save is not worth interrupting anyone for: the next turn saves again.
        crate::backend::spawn(async move { vivianne_runtime::store::save(&thread) });
    }

    /// Asks the model for a better title than the cleaned first sentence, without holding up the reply.
    fn ask_for_title(&mut self, first_ask: &str, cx: &mut Context<Self>) {
        self.title = Some((vivianne_runtime::title::fallback_title(first_ask), vivianne_runtime::title::AUTO));
        let Some(agent) = self.session.as_ref().ok().and_then(Session::agent) else { return };
        let (text, chat) = (first_ask.to_owned(), cx.entity().downgrade());
        crate::backend::run(cx, async move { agent.title(&text).await }, move |title, cx| {
            let Some(title) = title else { return };
            chat.update(cx, |chat, cx| {
                // A name the person gave while the model was thinking wins.
                if chat.title.as_ref().is_none_or(|(_, by)| *by != vivianne_runtime::title::USER) {
                    chat.title = Some((title, vivianne_runtime::title::AUTO));
                    cx.notify();
                }
            })
            .ok();
        });
    }

    #[must_use]
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Small status line under the composer.
    fn status_line(&self) -> impl IntoElement {
        let (status, dot) = if self.running { ("Working", theme::ACCENT) } else { ("Ready", theme::OK) };
        div()
            .pb(px(10.0))
            .flex()
            .justify_center()
            .items_center()
            .gap(px(7.0))
            .text_size(px(11.0))
            .text_color(color(theme::MUTED))
            .child(div().size(px(6.0)).flex_none().rounded_full().bg(color(dot)))
            .child(match self.thread_usage.as_ref().and_then(thread_usage::line) {
                Some(used) => format!("{}  ·  {status}  ·  {used}", self.selection.model),
                None => format!("{}  ·  {status}", self.selection.model),
            })
    }

    /// The screen before the first message: a question and a few starting points.
    fn empty_state(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .px(px(theme::SIDE_PAD))
            .child(
                div()
                    .w_full()
                    .max_w(px(theme::COLUMN_MAX))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .text_size(px(26.0))
                            .font_weight(FontWeight::MEDIUM)
                            .child("What are we working on?"),
                    )
                    .child(div().text_color(color(theme::MUTED)).child(format!(
                        "Working in {}. Ask a question, or have the agent change something.",
                        self.project
                    )))
                    .child(div().h(px(14.0)))
                    .children(SUGGESTIONS.iter().enumerate().map(|(index, text)| {
                        div()
                            .id(("suggestion", index))
                            .px(px(14.0))
                            .py(px(10.0))
                            .rounded(px(theme::RADIUS_SURFACE))
                            .border_1()
                            .border_color(color(theme::BORDER))
                            .cursor_pointer()
                            .text_size(px(13.0))
                            .hover(|row| row.bg(color(theme::SURFACE)))
                            .on_click(cx.listener(move |chat, _event, window, cx| {
                                chat.prefill(SUGGESTIONS[index], window, cx);
                            }))
                            .child(*text)
                    })),
            )
    }

    fn prefill(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.composer
            .input
            .update(cx, |input, cx| input.set_value(text, window, cx));
        window.focus(&self.composer.input.read(cx).focus_handle(cx), cx);
    }

    /// The conversation: a virtual list, so a long transcript costs no more to draw than a short one.
    fn transcript(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let chat = cx.entity();
        let row = StyleRefinement::default().max_w(px(theme::COLUMN_MAX)).mx_auto().pb(px(theme::GAP));
        let list = StyleRefinement::default().px(px(theme::SIDE_PAD));
        div().id("transcript").flex_1().min_h_0().child(
            MessageScroller::new("messages", self.scroller.clone(), move |index, _window, app| {
                chat.update(app, |chat, cx| match chat.items.get(index) {
                    Some(item) => transcript::render_item(index, item, &chat.folded_tools, cx),
                    None => div().into_any_element(),
                })
            })
            .with_row_style(row)
            .with_list_style(list),
        )
    }
}

impl Focusable for ChatView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.composer.input.read(cx).focus_handle(cx)
    }
}

impl Render for ChatView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(color(theme::BG))
            .text_color(color(theme::TEXT))
            .text_size(px(14.0))
            .child(if self.items.is_empty() {
                self.empty_state(cx).into_any_element()
            } else {
                self.transcript(cx).into_any_element()
            })
            .child(composer::render(self, window, cx))
            .child(self.status_line())
    }
}

const NEW_THREAD_TITLE: &str = "New thread";
const TITLE_MAX_CHARS: usize = 28;

/// Cut `text` to `max` characters on one line, adding an ellipsis when shortened.
fn shorten(text: &str, max: usize) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= max {
        return one_line;
    }
    let cut: String = one_line.chars().take(max).collect();
    format!("{}…", cut.trim_end())
}

#[cfg(test)]
mod tests {
    use super::shorten;

    #[test]
    fn short_text_is_kept_whole() {
        assert_eq!(shorten("fix the bug", 28), "fix the bug");
    }

    #[test]
    fn newlines_collapse_to_one_line() {
        assert_eq!(shorten("fix\n  the\nbug", 28), "fix the bug");
    }

    #[test]
    fn long_text_is_cut_with_an_ellipsis_on_characters_not_bytes() {
        assert_eq!(shorten("ééééé ééééé", 7), "ééééé é…");
    }
}
