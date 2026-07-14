mod app;
mod config;
mod herdr;
mod jira;
mod ui;

use app::{App, Resp};
use crossterm::event::{self, Event, KeyEventKind};
use std::sync::mpsc;
use std::time::Duration;

fn main() -> std::io::Result<()> {
    let (tx, rx) = mpsc::channel::<Resp>();
    let mut app = App::new(tx);

    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &mut app, rx);
    ratatui::restore();
    result
}

fn run(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    rx: mpsc::Receiver<Resp>,
) -> std::io::Result<()> {
    loop {
        terminal.draw(|f| ui::draw(f, app))?;

        // Drain background results first, then poll input briefly so the UI
        // stays responsive while requests are in flight.
        while let Ok(resp) = rx.try_recv() {
            app.on_resp(resp);
        }
        if event::poll(Duration::from_millis(120))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => app.on_key(key),
                _ => {}
            }
        }
        if app.should_quit {
            return Ok(());
        }
    }
}
