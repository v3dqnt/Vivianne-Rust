//! Phase 1 spike (c): can `alacritty_terminal` drive a real Windows pseudo
//! terminal? Runs a command, waits, and prints the emulated screen. Drawing the
//! grid is the same text drawing spike (a) already measures.

#![allow(clippy::pedantic, clippy::expect_used)] // throwaway spike

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::event_loop::EventLoop;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::tty::{self, Options, Shell};

const COLS: usize = 80;
const LINES: usize = 12;

struct Quiet;

impl EventListener for Quiet {
    fn send_event(&self, _event: Event) {}
}

struct Size;

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        LINES
    }
    fn screen_lines(&self) -> usize {
        LINES
    }
    fn columns(&self) -> usize {
        COLS
    }
}

fn main() {
    let options = Options {
        shell: Some(Shell::new(
            "cmd.exe".to_owned(),
            vec![
                "/C".to_owned(),
                "echo hello from a pty & echo second line & dir /b".to_owned(),
            ],
        )),
        working_directory: None,
        drain_on_exit: true,
        env: HashMap::new(),
        ..Options::default()
    };
    let window = WindowSize {
        num_lines: LINES as u16,
        num_cols: COLS as u16,
        cell_width: 8,
        cell_height: 16,
    };
    let pty = tty::new(&options, window, 0).expect("open pty");
    let term = Arc::new(FairMutex::new(Term::new(Config::default(), &Size, Quiet)));
    let _io = EventLoop::new(term.clone(), Quiet, pty, false, false)
        .expect("event loop")
        .spawn();

    std::thread::sleep(Duration::from_secs(2));

    let term = term.lock();
    for line in 0..LINES {
        let row: String = (0..COLS)
            .map(|col| term.grid()[Line(line as i32)][Column(col)].c)
            .collect();
        if !row.trim().is_empty() {
            println!("{}", row.trim_end());
        }
    }
}
