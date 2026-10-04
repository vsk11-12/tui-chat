use std::{io, time::Duration};

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

mod app;
mod events;
mod ollama;
mod ui;

use app::{App, Mode};

// ── Entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    // Start fetching the local model list in the background immediately
    app.load_models();

    let result = run_app(&mut terminal, &mut app).await;

    // Always restore the terminal, even on error
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    if let Err(e) = result {
        eprintln!("Fatal error: {e:?}");
    }

    Ok(())
}

// ── Main loop ─────────────────────────────────────────────────────────────────

async fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> Result<()> {
    const TICK: Duration = Duration::from_millis(16);

    loop {
        // 1. Render
        terminal.draw(|f| ui::render(f, app))?;

        // 2. Drain all queued background events (non-blocking)
        while let Ok(ev) = app.event_rx.try_recv() {
            app.handle_event(ev);
        }

        // 3. Poll for a keyboard / resize event with a short timeout
        if event::poll(TICK)? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    handle_key(app, key);
                }
                Event::Resize(_, _) => {
                    // ratatui redraws automatically on the next iteration
                }
                _ => {}
            }
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

// ── Key dispatch ──────────────────────────────────────────────────────────────

fn handle_key(app: &mut App, key: crossterm::event::KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    match app.mode {
        // ── Chat mode ─────────────────────────────────────────────────────────
        Mode::Chat => {
            if ctrl {
                match key.code {
                    // Quit — show confirmation instead of quitting immediately
                    KeyCode::Char('c') | KeyCode::Char('q') => {
                        app.mode = Mode::QuitConfirm;
                    }
                    // Clear chat history
                    KeyCode::Char('l') => app.clear_messages(),
                    // Open model picker
                    KeyCode::Char('m') => {
                        if !app.models.is_empty() {
                            app.open_model_select();
                        }
                    }
                    // Ctrl+W – delete word before cursor
                    KeyCode::Char('w') => app.delete_word_before(),
                    // Ctrl+A / Ctrl+E – line start / end
                    KeyCode::Char('a') => app.move_cursor_home(),
                    KeyCode::Char('e') => app.move_cursor_end(),
                    _ => {}
                }
            } else {
                match key.code {
                    // Esc: cancel a running stream; otherwise a no-op in chat
                    KeyCode::Esc => app.cancel_stream(),
                    KeyCode::Enter     => app.send_message(),
                    KeyCode::Char(c)   => app.insert_char(c),
                    KeyCode::Backspace => app.delete_char_before(),
                    KeyCode::Delete    => app.delete_char_after(),
                    KeyCode::Left      => app.move_cursor_left(),
                    KeyCode::Right     => app.move_cursor_right(),
                    KeyCode::Home      => app.move_cursor_home(),
                    KeyCode::End       => app.move_cursor_end(),
                    KeyCode::Up        => app.scroll_up(),
                    KeyCode::Down      => app.scroll_down(),
                    KeyCode::PageUp    => app.page_up(),
                    KeyCode::PageDown  => app.page_down(),
                    _ => {}
                }
            }
        }

        // ── Model-select popup ────────────────────────────────────────────────
        Mode::ModelSelect => match key.code {
            KeyCode::Esc => app.mode = Mode::Chat,
            KeyCode::Up   | KeyCode::Char('k') => app.model_select_up(),
            KeyCode::Down | KeyCode::Char('j') => app.model_select_down(),
            KeyCode::Enter => app.confirm_model_select(),
            _ => {}
        },

        // ── Quit confirmation dialog ──────────────────────────────────────────
        Mode::QuitConfirm => match key.code {
            // Confirm quit
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                app.should_quit = true;
            }
            // Cancel — return to chat
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                app.mode = Mode::Chat;
            }
            _ => {}
        },
    }
}
