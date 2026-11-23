//! One running shell: a pseudo terminal, the emulator that reads its output,
//! and a snapshot of the screen for drawing.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::tty::{self, Options, Shell};
use alacritty_terminal::vte::ansi::{Color, NamedColor};
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};

use super::screen::{Cell, DEFAULT_BG, DEFAULT_FG, Rgb, indexed};

/// What the shell side tells the view.
#[derive(Debug)]
pub enum Signal {
    /// The screen changed.
    Redraw,
    /// The program asked for this text to be sent back to it (answers to queries).
    Reply(String),
    /// The shell ended.
    Exited,
}

#[derive(Clone)]
struct Proxy(UnboundedSender<Signal>);

impl EventListener for Proxy {
    fn send_event(&self, event: Event) {
        let signal = match event {
            Event::Wakeup | Event::Title(_) | Event::ResetTitle => Signal::Redraw,
            Event::PtyWrite(text) => Signal::Reply(text),
            Event::Exit | Event::ChildExit(_) => Signal::Exited,
            _ => return,
        };
        let _ = self.0.unbounded_send(signal);
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

pub struct Session {
    term: Arc<FairMutex<Term<Proxy>>>,
    input: EventLoopSender,
    size: Size,
}

/// What to draw: the visible lines, and where the cursor sits among them.
pub struct Screen {
    pub lines: Vec<Vec<Cell>>,
}

impl Session {
    /// Starts the user's shell in `dir`. The signals arrive on the returned receiver.
    /// Starts the user's shell in `dir`, or `command` through it (a program and its arguments as one line, so
    /// `.cmd` shims and the PATH work as they do in a terminal; the shell stays open when it ends).
    pub fn start(
        dir: &Path,
        columns: u16,
        lines: u16,
        command: Option<&str>,
    ) -> Result<(Self, UnboundedReceiver<Signal>), String> {
        let size = Size { columns, lines };
        let (tx, rx) = unbounded();
        let options = Options {
            shell: Some(shell_for(command)),
            working_directory: Some(dir.to_path_buf()),
            drain_on_exit: true,
            env: HashMap::new(),
            ..Options::default()
        };
        let pty = tty::new(&options, window_size(size), 0)
            .map_err(|e| format!("Could not start a shell: {e}"))?;
        let term = Arc::new(FairMutex::new(Term::new(
            Config::default(),
            &size,
            Proxy(tx.clone()),
        )));
        let event_loop = EventLoop::new(term.clone(), Proxy(tx), pty, false, false)
            .map_err(|e| format!("Could not read the shell: {e}"))?;
        let input = event_loop.channel();
        event_loop.spawn();
        Ok((Self { term, input, size }, rx))
    }

    pub fn write(&self, bytes: Vec<u8>) {
        let _ = self.input.send(Msg::Input(Cow::Owned(bytes)));
    }

    /// Follows the view's size; a no-op when nothing changed.
    pub fn resize(&mut self, columns: u16, lines: u16) {
        let size = Size {
            columns: columns.max(2),
            lines: lines.max(1),
        };
        if size == self.size {
            return;
        }
        self.size = size;
        self.term.lock().resize(size);
        let _ = self.input.send(Msg::Resize(window_size(size)));
    }

    /// Scrolls back through output; positive goes toward older lines.
    pub fn scroll(&self, lines: i32) {
        self.term.lock().scroll_display(Scroll::Delta(lines));
    }

    pub fn scroll_to_bottom(&self) {
        self.term.lock().scroll_display(Scroll::Bottom);
    }

    /// Copies the visible screen out so the lock is not held while drawing.
    #[must_use]
    pub fn screen(&self) -> Screen {
        let term = self.term.lock();
        let (columns, lines) = (usize::from(self.size.columns), usize::from(self.size.lines));
        let content = term.renderable_content();
        let offset = i32::try_from(content.display_offset).unwrap_or(0);
        let mut grid = vec![vec![Cell::BLANK; columns]; lines];
        for indexed_cell in content.display_iter {
            let (Ok(row), column) = (
                usize::try_from(indexed_cell.point.line.0 + offset),
                indexed_cell.point.column.0,
            ) else {
                continue;
            };
            if indexed_cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            if let Some(slot) = grid.get_mut(row).and_then(|line| line.get_mut(column)) {
                *slot = to_cell(&indexed_cell);
            }
        }
        if content.display_offset == 0 {
            let at = content.cursor.point;
            let slot = usize::try_from(at.line.0)
                .ok()
                .and_then(|row| grid.get_mut(row))
                .and_then(|line| line.get_mut(at.column.0));
            if let Some(slot) = slot {
                *slot = inverted(*slot);
            }
        }
        Screen { lines: grid }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.input.send(Msg::Shutdown);
    }
}

fn window_size(size: Size) -> WindowSize {
    WindowSize {
        num_lines: size.lines,
        num_cols: size.columns,
        cell_width: 7,
        cell_height: 16,
    }
}

/// The shell to start: the user's own, running `command` first when there is one.
fn shell_for(command: Option<&str>) -> Shell {
    let Some(command) = command else {
        return default_shell();
    };
    if cfg!(windows) {
        Shell::new(
            "powershell.exe".to_owned(),
            vec![
                "-NoLogo".to_owned(),
                "-NoExit".to_owned(),
                "-Command".to_owned(),
                command.to_owned(),
            ],
        )
    } else {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned());
        Shell::new(
            shell,
            vec!["-c".to_owned(), format!("{command}; exec \"$SHELL\"")],
        )
    }
}

fn default_shell() -> Shell {
    if cfg!(windows) {
        Shell::new("powershell.exe".to_owned(), vec!["-NoLogo".to_owned()])
    } else {
        Shell::new(
            std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned()),
            vec![],
        )
    }
}

fn to_cell(
    cell: &alacritty_terminal::grid::Indexed<&alacritty_terminal::term::cell::Cell>,
) -> Cell {
    let flags = cell.flags;
    let (mut fg, mut bg) = (resolve(cell.fg, true), resolve(cell.bg, false));
    if flags.contains(Flags::INVERSE) {
        std::mem::swap(&mut fg, &mut bg);
    }
    Cell {
        ch: if flags.contains(Flags::HIDDEN) {
            ' '
        } else {
            cell.c
        },
        fg,
        bg: (bg != DEFAULT_BG).then_some(bg),
        bold: flags.contains(Flags::BOLD),
        underline: flags.intersects(Flags::ALL_UNDERLINES),
    }
}

/// The cursor is drawn by swapping the colours of the cell it sits on.
fn inverted(cell: Cell) -> Cell {
    Cell {
        fg: cell.bg.unwrap_or(DEFAULT_BG),
        bg: Some(cell.fg),
        ..cell
    }
}

fn resolve(color: Color, foreground: bool) -> Rgb {
    match color {
        Color::Spec(rgb) => (rgb.r, rgb.g, rgb.b),
        Color::Indexed(index) => indexed(index),
        Color::Named(named) => named_color(named, foreground),
    }
}

fn named_color(named: NamedColor, foreground: bool) -> Rgb {
    const FIRST_DIM: usize = NamedColor::DimBlack as usize;
    match named as usize {
        n @ 0..=15 => indexed(u8::try_from(n).unwrap_or(0)),
        n @ FIRST_DIM..=266 => indexed(u8::try_from(n - FIRST_DIM).unwrap_or(0)),
        _ if named == NamedColor::Background => DEFAULT_BG,
        _ if foreground => DEFAULT_FG,
        _ => DEFAULT_BG,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_colours_map_to_the_palette_and_the_defaults() {
        assert_eq!(named_color(NamedColor::Red, true), indexed(1));
        assert_eq!(named_color(NamedColor::BrightWhite, true), indexed(15));
        assert_eq!(named_color(NamedColor::DimGreen, true), indexed(2));
        assert_eq!(named_color(NamedColor::Foreground, true), DEFAULT_FG);
        assert_eq!(named_color(NamedColor::Background, false), DEFAULT_BG);
    }

    #[test]
    fn the_cursor_swaps_the_cell_colours() {
        let swapped = inverted(Cell::BLANK);
        assert_eq!(swapped.bg, Some(DEFAULT_FG));
        assert_eq!(swapped.fg, DEFAULT_BG);
    }
}
