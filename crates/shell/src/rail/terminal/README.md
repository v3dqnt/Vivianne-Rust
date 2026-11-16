# shell/src/rail/terminal

A real shell in the Terminal tab, built on `alacritty_terminal` (it runs the pseudo terminal and emulates the
screen).

- `mod.rs`: the `TerminalView`: draws the screen, takes key presses and scrolling, keeps the size in step with the
  window.
- `session.rs`: starts the shell (PowerShell on Windows, `$SHELL` elsewhere) in the project folder and copies the
  screen out for drawing.
- `screen.rs`: cells, colours and folding a line into styled spans. No UI types.
- `keys.rs`: key press to the bytes a program expects. No UI types.

Not done yet: selecting and copying text, pasting, mouse reporting for full-screen programs, and measuring the real
character size instead of the fixed estimate in `mod.rs`.
