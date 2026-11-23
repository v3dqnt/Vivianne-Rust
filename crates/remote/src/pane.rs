//! A terminal pane that lives on the server: a pseudo terminal, the emulator that reads its output, and the screen
//! as text. The server owns it, so it keeps running when no client is attached; a client in view is sent the
//! screen as lines, and every client is told what the agent in it is doing (read off the screen, see `detect`).

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::tty::{self, Options, Shell};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// What a pane tells the server.
#[derive(Debug)]
pub enum Signal {
    /// The screen changed.
    Redraw,
    /// The program asked for this text to be sent back to it (answers to queries).
    Reply(String),
    Title(String),
    /// The shell ended.
    Exited,
}

#[derive(Clone)]
struct Proxy(UnboundedSender<Signal>);

impl EventListener for Proxy {
    fn send_event(&self, event: Event) {
        let signal = match event {
            Event::Wakeup => Signal::Redraw,
            Event::Title(t) => Signal::Title(t),
            Event::ResetTitle => Signal::Title(String::new()),
            Event::PtyWrite(text) => Signal::Reply(text),
            Event::Exit | Event::ChildExit(_) => Signal::Exited,
            _ => return,
        };
        let _ = self.0.send(signal);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Size {
    columns: u16,
    lines: u16,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        usize::from(self.lines)
    }
    fn screen_lines(&self) -> usize {
        usize::from(self.lines)
    }
    fn columns(&self) -> usize {
        usize::from(self.columns)
    }
}

/// How many lines of history detection and a client's scrollback look at.
pub const SCROLLBACK_LINES: usize = 2000;

pub struct Pane {
    term: Arc<FairMutex<Term<Proxy>>>,
    input: EventLoopSender,
    size: std::sync::Mutex<Size>,
}

impl std::fmt::Debug for Pane {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Pane")
    }
}

/// The screen as text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Frame {
    pub lines: Vec<String>,
    /// The cursor's row and column.
    pub cursor: (u16, u16),
}

fn window_size(size: Size) -> WindowSize {
    WindowSize {
        num_lines: size.lines,
        num_cols: size.columns,
        cell_width: 7,
        cell_height: 16,
    }
}

/// The shell to start: the machine's own, running `command` first when there is one (the shell stays open after).
fn shell_for(command: Option<&str>) -> Shell {
    match (command, cfg!(windows)) {
        (None, true) => Shell::new("powershell.exe".to_owned(), vec!["-NoLogo".to_owned()]),
        (Some(c), true) => Shell::new(
            "powershell.exe".to_owned(),
            ["-NoLogo", "-NoExit", "-Command", c]
                .map(str::to_owned)
                .to_vec(),
        ),
        (None, false) => Shell::new(
            std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned()),
            vec![],
        ),
        (Some(c), false) => Shell::new(
            std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned()),
            vec![
                "-c".to_owned(),
                format!("{c}; exec \"${{SHELL:-/bin/sh}}\""),
            ],
        ),
    }
}

impl Pane {
    /// Starts a pane in `dir`.
    pub fn start(
        dir: &Path,
        columns: u16,
        lines: u16,
        command: Option<&str>,
        extra_env: HashMap<String, String>,
    ) -> Result<(Self, UnboundedReceiver<Signal>), String> {
        let size = Size {
            columns: columns.max(2),
            lines: lines.max(1),
        };
        let (tx, rx) = unbounded_channel();
        let options = Options {
            shell: Some(shell_for(command)),
            working_directory: Some(dir.to_path_buf()),
            drain_on_exit: true,
            env: extra_env,
            ..Options::default()
        };
        let pty = tty::new(&options, window_size(size), 0)
            .map_err(|e| format!("could not start a shell: {e}"))?;
        let term = Arc::new(FairMutex::new(Term::new(
            Config::default(),
            &size,
            Proxy(tx.clone()),
        )));
        let event_loop = EventLoop::new(term.clone(), Proxy(tx), pty, false, false)
            .map_err(|e| format!("could not read the shell: {e}"))?;
        let input = event_loop.channel();
        event_loop.spawn();
        Ok((
            Self {
                term,
                input,
                size: std::sync::Mutex::new(size),
            },
            rx,
        ))
    }

    pub fn write(&self, bytes: Vec<u8>) {
        let _ = self.input.send(Msg::Input(Cow::Owned(bytes)));
    }

    /// Follows the viewer's size; nothing happens when it did not change.
    pub fn resize(&self, columns: u16, lines: u16) {
        let size = Size {
            columns: columns.max(2),
            lines: lines.max(1),
        };
        let mut current = self
            .size
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if size == *current {
            return;
        }
        *current = size;
        drop(current);
        self.term.lock().resize(size);
        let _ = self.input.send(Msg::Resize(window_size(size)));
    }

    /// The visible screen as lines of text, trailing blanks trimmed.
    #[must_use]
    pub fn frame(&self) -> Frame {
        let term = self.term.lock();
        let size = *self
            .size
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (columns, lines) = (usize::from(size.columns), usize::from(size.lines));
        let content = term.renderable_content();
        let mut grid = vec![vec![' '; columns]; lines];
        for cell in content.display_iter {
            let (Ok(row), column) = (usize::try_from(cell.point.line.0), cell.point.column.0)
            else {
                continue;
            };
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) || cell.flags.contains(Flags::HIDDEN) {
                continue;
            }
            if let Some(slot) = grid.get_mut(row).and_then(|l| l.get_mut(column)) {
                *slot = cell.c;
            }
        }
        let at = content.cursor.point;
        Frame {
            lines: grid
                .into_iter()
                .map(|l| l.into_iter().collect::<String>().trim_end().to_owned())
                .collect(),
            cursor: (
                u16::try_from(at.line.0).unwrap_or(0),
                u16::try_from(at.column.0).unwrap_or(0),
            ),
        }
    }
}

impl Drop for Pane {
    fn drop(&mut self) {
        let _ = self.input.send(Msg::Shutdown);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_pane_runs_a_command_and_its_output_is_read_back_as_text() {
        let dir = std::env::temp_dir();
        let (pane, mut signals) =
            Pane::start(&dir, 80, 12, Some("echo viv-pane-works"), HashMap::new()).expect("starts");
        let found = tokio::time::timeout(std::time::Duration::from_secs(20), async {
            loop {
                if pane
                    .frame()
                    .lines
                    .iter()
                    .any(|l| l.contains("viv-pane-works") && !l.contains("echo"))
                {
                    return true;
                }
                // Wait for the screen to change before looking again.
                if signals.recv().await.is_none() {
                    return false;
                }
            }
        })
        .await
        .unwrap_or(false);
        assert!(found, "the command's output should reach the screen");
    }
}
