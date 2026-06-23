use std::{io, sync::Arc, time::Duration};

use anyhow::Result;
use crossterm::{
    event::KeyCode,
    event::{DisableBracketedPaste, EnableBracketedPaste},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::{Backend, CrosstermBackend},
};

use crate::state::AppState;

use super::{
    events::drain_job_events,
    frame::{punctuation_test_page_count, render, render_punctuation_test_page},
    input::{TuiInputEvent, spawn_tui_input_reader},
    models::IMPORTANT_STATUS_PIN,
    state::TuiApp,
};

const TUI_PUNCTUATION_TEST_ENV: &str = "CLASH_TUI_PUNCTUATION_TEST";

pub async fn run(state: Arc<AppState>) -> Result<()> {
    state.metrics.start();
    if let Ok(value) = std::env::var(crate::terminal_display::TUI_DISPLAY_MODE_ENV)
        && let Some(mode) = crate::terminal_display::parse_display_mode(Some(&value))
    {
        crate::terminal_display::set_current_display_mode(mode);
    }
    if let Ok(value) = std::env::var(crate::terminal_display::TUI_PUNCTUATION_MODE_ENV)
        && let Some(mode) = crate::terminal_display::parse_punctuation_mode(Some(&value))
    {
        crate::terminal_display::set_current_punctuation_mode(mode);
    }
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let result = if punctuation_test_enabled() {
        run_punctuation_test_loop(&mut terminal).await
    } else {
        run_loop(&mut terminal, state).await
    };
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), DisableBracketedPaste, LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn punctuation_test_enabled() -> bool {
    std::env::var(TUI_PUNCTUATION_TEST_ENV)
        .ok()
        .map(|value| {
            let value = value.trim().to_ascii_lowercase();
            matches!(value.as_str(), "1" | "true" | "yes" | "on")
        })
        .unwrap_or(false)
}

async fn run_punctuation_test_loop<B>(terminal: &mut Terminal<B>) -> Result<()>
where
    B: Backend,
{
    let (mut input, _input_guard) = spawn_tui_input_reader();
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    let mut page = 0_usize;
    loop {
        terminal.draw(|frame| {
            render_punctuation_test_page(frame.area(), frame.buffer_mut(), page);
        })?;
        tokio::select! {
            maybe_event = input.recv() => {
                match maybe_event {
                    Some(TuiInputEvent::Key(KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc)) | None => break,
                    Some(TuiInputEvent::Key(KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::PageDown | KeyCode::Right | KeyCode::Down)) => {
                        let page_count = punctuation_test_page_count();
                        page = (page + 1).min(page_count.saturating_sub(1));
                    }
                    Some(TuiInputEvent::Key(KeyCode::Char('p') | KeyCode::Char('P') | KeyCode::PageUp | KeyCode::Left | KeyCode::Up)) => {
                        page = page.saturating_sub(1);
                    }
                    Some(TuiInputEvent::Paste(_)) | Some(TuiInputEvent::Key(_)) => {}
                }
            }
            _ = tick.tick() => {}
        }
    }
    Ok(())
}

async fn run_loop<B>(terminal: &mut Terminal<B>, state: Arc<AppState>) -> Result<()>
where
    B: Backend,
{
    let mut app = TuiApp::default();
    let mut last_rendered_view = app.view;
    let mut events = state.jobs.subscribe();
    let (mut input, _input_guard) = spawn_tui_input_reader();
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    loop {
        if let Ok(input_event) = input.try_recv() {
            if handle_input_event(terminal, &mut app, &state, input_event).await? {
                return Ok(());
            }
            draw_app_frame(terminal, &mut app, &state, &mut last_rendered_view)?;
            continue;
        }
        app.refresh(&state).await;
        drain_job_events(&mut app, &mut events);
        draw_app_frame(terminal, &mut app, &state, &mut last_rendered_view)?;

        tokio::select! {
            maybe_event = input.recv() => {
                match maybe_event {
                    Some(input_event) => {
                        if handle_input_event(terminal, &mut app, &state, input_event).await? {
                            break;
                        }
                    }
                    None => break,
                }
            }
            _ = tick.tick() => {}
        }
    }
    Ok(())
}

fn draw_app_frame<B>(
    terminal: &mut Terminal<B>,
    app: &mut TuiApp,
    state: &Arc<AppState>,
    last_rendered_view: &mut super::models::View,
) -> Result<()>
where
    B: Backend,
{
    if app.view != *last_rendered_view {
        terminal.clear()?;
        *last_rendered_view = app.view;
    }
    app.sync_dashboard_metrics(state);
    terminal.draw(|frame| {
        render(frame.area(), frame.buffer_mut(), app, state);
    })?;
    Ok(())
}

async fn handle_input_event<B>(
    terminal: &mut Terminal<B>,
    app: &mut TuiApp,
    state: &Arc<AppState>,
    input_event: TuiInputEvent,
) -> Result<bool>
where
    B: Backend,
{
    match input_event {
        TuiInputEvent::Key(code) => {
            let busy_message = app.busy_message_for_key(code);
            if let Some(message) = busy_message {
                app.start_busy(message);
                terminal.draw(|frame| {
                    render(frame.area(), frame.buffer_mut(), app, state);
                })?;
            }
            let handled = app.handle_key(code, state).await;
            app.clear_busy();
            let should_exit = handled?;
            if busy_message.is_some() && !should_exit {
                app.pin_status(IMPORTANT_STATUS_PIN);
            }
            Ok(should_exit)
        }
        TuiInputEvent::Paste(value) => Ok(app.handle_paste(value)),
    }
}
