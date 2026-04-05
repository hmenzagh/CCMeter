mod app;
mod config;
mod data;
mod ui;

use std::io;
use std::time::Duration;

use clap::Parser;
use crossterm::{
    ExecutableCommand,
    event::{self, Event as CrosstermEvent},
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::prelude::*;

#[derive(Parser)]
#[command(name = "ccmeter", about = "Claude Code usage statistics")]
struct Cli {}

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = io::stdout().execute(LeaveAlternateScreen);
    }
}

fn main() -> io::Result<()> {
    let _cli = Cli::parse();

    let mut app = app::App::new();

    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = io::stdout().execute(LeaveAlternateScreen);
        original_hook(info);
    }));

    enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;
    let _guard = TerminalGuard;

    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let poll_timeout = Duration::from_millis(120);

    loop {
        app.pre_render();
        terminal.draw(|frame| app.draw(frame))?;
        app.handle_reload();

        if !event::poll(poll_timeout)? {
            continue;
        }

        if let CrosstermEvent::Key(key) = event::read()?
            && !app.handle_input(key)
        {
            break;
        }
    }

    Ok(())
}
