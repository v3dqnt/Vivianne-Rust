//! One persistent shell on a pseudo terminal, with `alacritty_terminal` reading its output.
//!
//! Port of `@deepseek-ai/dsh-terminal-bash/session.ts`. The emulator stands in for the original's sanitiser, and
//! "waiting for input" is inferred from silence only.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::tty::{self, Options, Shell};

const COLUMNS: u16 = 160;
const ROWS: u16 = 40;
const SCROLLBACK_LINES: usize = 10_000;
const POLL: Duration = Duration::from_millis(50);
/// Output quiet this long after an input means the program is waiting.
const IDLE: Duration = Duration::from_secs(3);
/// A program that has said nothing at all yet is given this much longer before it counts as idle.
const FIRST_OUTPUT: Duration = Duration::from_secs(10);
const SEND_TIMEOUT: Duration = Duration::from_secs(30);

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the reader thread learns.
#[derive(Default)]
struct Seen {
    input: OnceLock<EventLoopSender>,
    last_output: Mutex<Option<Instant>>,
    exited: AtomicBool,
    exit_code: Mutex<Option<i32>>,
}

#[derive(Clone, Default)]
struct Listener(Arc<Seen>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        match event {
            Event::Wakeup => *lock(&self.0.last_output) = Some(Instant::now()),
            // A query the terminal must answer, or the program hangs.
            Event::PtyWrite(text) => {
                if let Some(input) = self.0.input.get() {
                    let _ = input.send(Msg::Input(Cow::Owned(text.into_bytes())));
                }
            }
            Event::ChildExit(code) => {
                *lock(&self.0.exit_code) = Some(code);
                self.0.exited.store(true, Ordering::SeqCst);
            }
            Event::Exit => self.0.exited.store(true, Ordering::SeqCst),
            _ => {}
        }
    }
}

struct Size;

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        usize::from(ROWS)
    }
    fn screen_lines(&self) -> usize {
        usize::from(ROWS)
    }
    fn columns(&self) -> usize {
        usize::from(COLUMNS)
    }
}

/// Why a send stopped waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wait {
    InferredIdle,
    Timeout,
    SessionExit,
}

impl Wait {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InferredIdle => "inferred_idle",
            Self::Timeout => "timeout",
            Self::SessionExit => "session_exit",
        }
    }
}

pub struct Session {
    term: Arc<FairMutex<Term<Listener>>>,
    seen: Arc<Seen>,
    input: EventLoopSender,
    /// Held while a send waits: one at a time.
    busy: AtomicBool,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("exited", &self.exited())
            .finish_non_exhaustive()
    }
}

/// Releases the one send a session allows.
pub struct Sending(Arc<Session>);

impl Drop for Sending {
    fn drop(&mut self) {
        self.0.busy.store(false, Ordering::SeqCst);
    }
}

/// Secrets of this process, blanked for the shell: the pseudo terminal can override a variable but not remove it.
fn blanked() -> HashMap<String, String> {
    std::env::vars_os()
        .filter_map(|(name, _)| name.into_string().ok())
        .filter(|name| vivianne_sandbox::env::withheld(name.as_ref()))
        .map(|name| (name, String::new()))
        .collect()
}

impl Session {
    /// Starts `argv` in `dir`.
    pub fn start(argv: &[String], dir: &Path) -> Result<Self, String> {
        let (program, rest) = argv.split_first().ok_or("there is no shell to run")?;
        let options = Options {
            shell: Some(Shell::new(program.clone(), rest.to_vec())),
            working_directory: Some(dir.to_path_buf()),
            drain_on_exit: true,
            env: blanked(),
            ..Options::default()
        };
        let size = WindowSize {
            num_lines: ROWS,
            num_cols: COLUMNS,
            cell_width: 7,
            cell_height: 16,
        };
        let pty =
            tty::new(&options, size, 0).map_err(|e| format!("could not start the shell: {e}"))?;
        let listener = Listener::default();
        let seen = Arc::clone(&listener.0);
        let config = Config {
            scrolling_history: SCROLLBACK_LINES,
            ..Config::default()
        };
        let term = Arc::new(FairMutex::new(Term::new(config, &Size, listener.clone())));
        let reader = EventLoop::new(Arc::clone(&term), listener, pty, true, false)
            .map_err(|e| format!("could not read the shell: {e}"))?;
        let input = reader.channel();
        let _ = seen.input.set(input.clone());
        reader.spawn();
        Ok(Self {
            term,
            seen,
            input,
            busy: AtomicBool::new(false),
        })
    }

    pub fn exited(&self) -> bool {
        self.seen.exited.load(Ordering::SeqCst)
    }

    pub fn exit_code(&self) -> Option<i32> {
        *lock(&self.seen.exit_code)
    }

    /// The one send this session allows at a time.
    pub fn begin_send(self: &Arc<Self>) -> Option<Sending> {
        (!self.busy.swap(true, Ordering::SeqCst)).then(|| Sending(Arc::clone(self)))
    }

    pub fn write(&self, bytes: Vec<u8>) {
        let _ = self.input.send(Msg::Input(Cow::Owned(bytes)));
    }

    /// Waits until the output goes quiet, the shell exits or the time is up. `since` is when the input went in.
    pub async fn settle(&self, since: Instant) -> Wait {
        loop {
            tokio::time::sleep(POLL).await;
            if self.exited() {
                return Wait::SessionExit;
            }
            let answered = lock(&self.seen.last_output).filter(|at| *at >= since);
            match answered {
                Some(at) if at.elapsed() >= IDLE => return Wait::InferredIdle,
                None if since.elapsed() >= FIRST_OUTPUT => return Wait::InferredIdle,
                _ => {}
            }
            if since.elapsed() >= SEND_TIMEOUT {
                return Wait::Timeout;
            }
        }
    }

    fn lines(&self, from_history: bool) -> Vec<String> {
        let term = self.term.lock();
        let grid = term.grid();
        let history = if from_history { grid.history_size() } else { 0 };
        let first = -i32::try_from(history).unwrap_or(i32::MAX);
        let last = i32::from(ROWS) - 1;
        let text = term.bounds_to_string(
            Point::new(Line(first), Column(0)),
            Point::new(Line(last), Column(usize::from(COLUMNS) - 1)),
        );
        let mut lines: Vec<String> = text
            .lines()
            .map(|line| line.trim_end().to_owned())
            .collect();
        // The empty rows below the cursor are not output.
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        lines
    }

    /// What is on the screen now.
    pub fn viewport(&self) -> String {
        self.lines(false).join("\n")
    }

    /// A page of what is retained, counted back from the newest line: the text, how many lines there are, and the
    /// range (first, one past last) returned.
    pub fn read(&self, offset: usize, count: usize) -> (String, usize, usize, usize) {
        let lines = self.lines(true);
        let total = lines.len();
        let end = total.saturating_sub(offset);
        let begin = end.saturating_sub(count);
        (lines[begin..end].join("\n"), total, begin, end)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Closing the pseudo terminal ends what is attached to it.
        let _ = self.input.send(Msg::Shutdown);
    }
}
