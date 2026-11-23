//! The terminal panes a server owns. Each is a shell (or a program run through one) in a folder; it keeps running
//! when no client is attached. For every pane the server watches the screen: it sends the lines to the clients
//! in view, and reads which coding agent is running and whether it is working, waiting for the person, or idle.
//! The panes are saved on every change, and reopened (as fresh shells in the same folders) after a full stop.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::broadcast;

use crate::detect::{self, Screen};
use crate::pane::{Frame, Pane, Signal};
use crate::persist::{self, SavedPane, SessionFile};
use crate::protocol::{AgentState, PaneInfo, ServerMessage};

/// A burst of screen changes is drawn once after this long.
const REDRAW_DEBOUNCE: Duration = Duration::from_millis(30);
/// Most panes one server keeps.
pub const MAX_PANES: usize = 32;

struct Entry {
    pane: Arc<Pane>,
    info: PaneInfo,
}

pub struct Panes {
    session: String,
    entries: Mutex<HashMap<String, Entry>>,
    events: broadcast::Sender<ServerMessage>,
    ssh_agent: Mutex<Option<String>>,
}

impl std::fmt::Debug for Panes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Panes")
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The folder a pane starts in: the one asked for, which must exist here, else this machine's home folder.
fn start_dir(workspace: &str) -> Result<PathBuf, String> {
    if workspace.trim().is_empty() {
        return std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
            .ok_or_else(|| "this machine has no home folder".to_owned());
    }
    let dir = PathBuf::from(workspace);
    if dir.is_dir() {
        Ok(dir)
    } else {
        Err(format!("{workspace} is not a folder on this machine"))
    }
}

/// What a pane's agent is doing, from its screen: the manifest of the recognised agent, else unknown.
#[must_use]
pub fn state_of(agent: Option<&str>, frame: &Frame, title: &str) -> AgentState {
    let Some(manifest) = agent.and_then(detect::manifest_for) else {
        return AgentState::Unknown;
    };
    let screen = Screen {
        lines: frame.lines.clone(),
        osc_title: title.to_owned(),
    };
    manifest.detect(&screen).state
}

impl Panes {
    #[must_use]
    pub fn new(session: &str, events: broadcast::Sender<ServerMessage>) -> Arc<Self> {
        Arc::new(Self {
            session: session.to_owned(),
            entries: Mutex::new(HashMap::new()),
            events,
            ssh_agent: Mutex::new(None),
        })
    }

    #[must_use]
    pub fn infos(&self) -> Vec<PaneInfo> {
        let mut infos: Vec<PaneInfo> = self
            .entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .map(|e| e.info.clone())
            .collect();
        infos.sort_by(|a, b| a.id.cmp(&b.id));
        infos
    }

    fn persist(&self) {
        let panes = self
            .infos()
            .into_iter()
            .map(|i| SavedPane {
                id: i.id,
                workspace: i.workspace,
                command: i.command,
            })
            .collect();
        if let Err(why) = persist::save(
            &crate::socket::session_dir(&self.session),
            &SessionFile {
                version: persist::FORMAT_VERSION,
                panes,
            },
        ) {
            vivianne_core::debuglog::log("remote", format!("could not save the session: {why}"));
        }
    }

    /// Opens a pane. The id is the client's, so a client can find the pane it opened.
    pub fn open(
        self: &Arc<Self>,
        id: &str,
        workspace: &str,
        command: Option<&str>,
        cols: u16,
        rows: u16,
    ) -> Result<(), String> {
        if !valid_id(id) {
            return Err("not a pane id".to_owned());
        }
        if self
            .entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
            >= MAX_PANES
        {
            return Err(format!("this server keeps at most {MAX_PANES} panes"));
        }
        if self
            .entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains_key(id)
        {
            return Err("a pane with that id is already open".to_owned());
        }
        let dir = start_dir(workspace)?;
        let command = command.map(str::trim).filter(|c| !c.is_empty());
        let mut env = HashMap::new();
        if let Some(socket) = self
            .ssh_agent
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
        {
            env.insert("SSH_AUTH_SOCK".to_owned(), socket);
        }
        let (pane, signals) = Pane::start(&dir, cols.max(20), rows.max(5), command, env)?;
        let agent = command
            .and_then(|c| {
                detect::identify_agent(&c.split_whitespace().map(str::to_owned).collect::<Vec<_>>())
            })
            .map(str::to_owned);
        let info = PaneInfo {
            id: id.to_owned(),
            workspace: dir.display().to_string(),
            command: command.map(str::to_owned),
            agent: agent.clone(),
            state: AgentState::Unknown,
            title: String::new(),
        };
        let pane = Arc::new(pane);
        self.entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                id.to_owned(),
                Entry {
                    pane: pane.clone(),
                    info,
                },
            );
        self.persist();
        let panes = self.clone();
        let id = id.to_owned();
        tokio::spawn(async move { panes.watch(id, pane, agent, signals).await });
        Ok(())
    }

    /// Follows one pane until its shell ends.
    async fn watch(
        self: Arc<Self>,
        id: String,
        pane: Arc<Pane>,
        agent: Option<String>,
        mut signals: tokio::sync::mpsc::UnboundedReceiver<Signal>,
    ) {
        let mut title = String::new();
        let mut last_state = AgentState::Unknown;
        let mut last_frame = Frame::default();
        while let Some(signal) = signals.recv().await {
            match signal {
                Signal::Reply(text) => pane.write(text.into_bytes()),
                Signal::Title(t) => title = t,
                Signal::Exited => break,
                Signal::Redraw => {}
            }
            // Take whatever else arrived during the debounce, so a burst is one frame.
            tokio::time::sleep(REDRAW_DEBOUNCE).await;
            while let Ok(next) = signals.try_recv() {
                match next {
                    Signal::Reply(text) => pane.write(text.into_bytes()),
                    Signal::Title(t) => title = t,
                    Signal::Exited => {
                        self.closed(&id);
                        return;
                    }
                    Signal::Redraw => {}
                }
            }
            let frame = pane.frame();
            if frame != last_frame {
                let _ = self.events.send(ServerMessage::PaneFrame {
                    pane: id.clone(),
                    lines: frame.lines.clone(),
                    cursor: frame.cursor,
                    title: title.clone(),
                });
                let state = state_of(agent.as_deref(), &frame, &title);
                if state != last_state {
                    last_state = state;
                    if let Some(entry) = self
                        .entries
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .get_mut(&id)
                    {
                        entry.info.state = state;
                        entry.info.title.clone_from(&title);
                    }
                    let _ = self.events.send(ServerMessage::PaneState {
                        pane: id.clone(),
                        agent: agent.clone(),
                        state,
                    });
                    if state == AgentState::Blocked {
                        let _ = self.events.send(ServerMessage::Notify {
                            title: "Waiting for you".to_owned(),
                            body: format!("pane {id}"),
                        });
                    }
                }
                last_frame = frame;
            }
        }
        self.closed(&id);
    }

    fn closed(&self, id: &str) {
        if self
            .entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id)
            .is_some()
        {
            let _ = self.events.send(ServerMessage::PaneClosed {
                pane: id.to_owned(),
            });
            self.persist();
        }
    }

    pub fn input(&self, id: &str, data: &str) {
        if let Some(e) = self
            .entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
        {
            e.pane.write(data.as_bytes().to_vec());
        }
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) {
        if let Some(e) = self
            .entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
        {
            e.pane.resize(cols.max(20), rows.max(5));
        }
    }

    /// Closes a pane by dropping it, which ends its shell.
    pub fn close(&self, id: &str) {
        self.closed(id);
    }

    /// The SSH agent socket panes opened after this get. It must exist on this machine.
    pub fn set_ssh_agent(&self, socket: &str) -> Result<(), String> {
        if !std::path::Path::new(socket).exists() {
            return Err("that SSH agent socket does not exist on this machine".to_owned());
        }
        *self
            .ssh_agent
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(socket.to_owned());
        Ok(())
    }

    /// Reopens the panes saved before the last stop, as fresh shells in their folders.
    pub fn restore(self: &Arc<Self>) {
        let (saved, problem) = persist::load(&crate::socket::session_dir(&self.session));
        if let Some(why) = problem {
            vivianne_core::debuglog::log("remote", why);
        }
        for p in saved.panes {
            // The program is not run again: only the shell in its folder comes back.
            if let Err(why) = self.open(&p.id, &p.workspace, None, 100, 30) {
                vivianne_core::debuglog::log(
                    "remote",
                    format!("pane {} was not restored: {why}", p.id),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_ids_are_plain_words_and_the_folder_must_exist_here() {
        assert!(valid_id("p-1_a") && !valid_id("") && !valid_id("../x") && !valid_id("a b"));
        assert!(
            start_dir("/no/such/folder/anywhere/at/all")
                .unwrap_err()
                .contains("not a folder")
        );
        assert!(start_dir("").is_ok());
        assert!(start_dir(&std::env::temp_dir().display().to_string()).is_ok());
    }

    #[test]
    fn a_pane_without_a_known_agent_is_unknown_and_one_with_an_agent_is_read_off_its_screen() {
        let frame = Frame {
            lines: vec!["Do you want to proceed? (y/n)".to_owned()],
            cursor: (0, 0),
        };
        assert_eq!(state_of(None, &frame, ""), AgentState::Unknown);
        assert_eq!(
            state_of(Some("no-such-agent"), &frame, ""),
            AgentState::Unknown
        );
        assert_eq!(state_of(Some("claude"), &frame, ""), AgentState::Blocked);
        let quiet = Frame {
            lines: vec!["> ".to_owned()],
            cursor: (0, 2),
        };
        assert_eq!(state_of(Some("claude"), &quiet, ""), AgentState::Idle);
    }

    #[tokio::test]
    async fn a_pane_opens_shows_its_output_to_the_clients_and_closes() {
        let (events, mut rx) = broadcast::channel(256);
        let panes = Panes::new(&format!("t{}", uuid::Uuid::new_v4().simple()), events);
        panes
            .open(
                "p1",
                &std::env::temp_dir().display().to_string(),
                Some("echo viv-pane-ok"),
                80,
                12,
            )
            .expect("opens");
        assert!(
            panes.open("p1", "", None, 80, 12).is_err(),
            "ids are unique"
        );
        let saw = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if let Ok(ServerMessage::PaneFrame { lines, .. }) = rx.recv().await
                    && lines
                        .iter()
                        .any(|l| l.contains("viv-pane-ok") && !l.contains("echo"))
                {
                    return true;
                }
            }
        })
        .await
        .unwrap_or(false);
        assert!(saw, "the output should arrive as a frame");
        assert_eq!(panes.infos().len(), 1);
        panes.close("p1");
        assert!(panes.infos().is_empty());
        let _ = std::fs::remove_dir_all(crate::socket::session_dir(&panes.session));
    }
}
