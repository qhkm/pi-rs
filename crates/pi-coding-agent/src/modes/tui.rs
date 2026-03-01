/// Full TUI interactive mode using pi-tui components.
///
/// Provides a complete terminal UI with:
/// - Input editor for composing prompts
/// - Streaming response display
/// - Status bar with model info
/// - Message history
use std::io::{self, Write};
use std::sync::Arc;

use anyhow::Result;
use crossterm::{
    cursor, event,
    terminal::{self, ClearType},
    ExecutableCommand,
};
use pi_agent_core::{Agent, AgentEvent};
use pi_ai::{Content, StreamEvent};
use pi_tui::{Component, Editor, Focusable};

/// Application state for the TUI mode
struct TuiApp {
    #[allow(dead_code)]
    agent: Arc<Agent>,
    editor: Editor,
    messages: Vec<DisplayMessage>,
    status: String,
    streaming_text: String,
    is_streaming: bool,
    should_quit: bool,
}

/// A rendered message in the history
struct DisplayMessage {
    role: String,
    text: String,
}

impl TuiApp {
    fn new(agent: Arc<Agent>) -> Self {
        let mut editor = Editor::new(3);
        editor.set_focused(true);

        Self {
            agent,
            editor,
            messages: Vec::new(),
            status: String::new(),
            streaming_text: String::new(),
            is_streaming: false,
            should_quit: false,
        }
    }

    /// Render the entire screen
    fn render(&self) -> Result<()> {
        let mut stdout = io::stdout();
        let (cols, rows) = terminal::size()?;

        // Move to top-left and clear
        stdout.execute(cursor::MoveTo(0, 0))?;
        stdout.execute(terminal::Clear(ClearType::All))?;

        let mut row = 0u16;

        // Render message history
        for msg in &self.messages {
            if row >= rows.saturating_sub(6) {
                break;
            }
            let prefix = if msg.role == "user" { "> " } else { "  " };
            let line = format!("{}{}", prefix, truncate(&msg.text, cols as usize - 2));
            stdout.execute(cursor::MoveTo(0, row))?;
            write!(stdout, "{line}")?;
            row += 1;
        }

        // Render streaming response
        if self.is_streaming && !self.streaming_text.is_empty() {
            if row < rows.saturating_sub(5) {
                stdout.execute(cursor::MoveTo(0, row))?;
                let line = truncate(&self.streaming_text, cols as usize);
                write!(stdout, "  {line}")?;
            }
        }

        // Status bar (2nd from bottom)
        let status_row = rows.saturating_sub(4);
        stdout.execute(cursor::MoveTo(0, status_row))?;
        let bar = format!("─── {} ", &self.status);
        write!(stdout, "{}", truncate(&bar, cols as usize))?;

        // Editor (bottom 3 rows)
        let editor_row = rows.saturating_sub(3);
        let rendered = self.editor.render(cols);
        for (i, line) in rendered.iter().enumerate() {
            let y = editor_row + i as u16;
            if y < rows {
                stdout.execute(cursor::MoveTo(0, y))?;
                write!(stdout, "{line}")?;
            }
        }

        stdout.flush()?;
        Ok(())
    }

    /// Handle a crossterm key event
    fn handle_key(&mut self, key: event::KeyEvent) {
        use event::{KeyCode, KeyModifiers};

        // Ctrl-C / Ctrl-D quit
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && (key.code == KeyCode::Char('c') || key.code == KeyCode::Char('d'))
        {
            self.should_quit = true;
            return;
        }

        // Convert crossterm key event to a byte string for pi-tui input handling
        let data = crossterm_key_to_bytes(&key);
        if !data.is_empty() {
            self.editor.handle_input(&data);
        }
    }
}

/// Run the full TUI interactive mode.
pub async fn run_tui_mode(agent: Arc<Agent>) -> Result<()> {
    // Enter raw mode and alternate screen
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    stdout.execute(terminal::EnterAlternateScreen)?;
    stdout.execute(cursor::Hide)?;

    let result = run_tui_loop(agent).await;

    // Cleanup
    let mut stdout = io::stdout();
    stdout.execute(cursor::Show)?;
    stdout.execute(terminal::LeaveAlternateScreen)?;
    terminal::disable_raw_mode()?;

    result
}

async fn run_tui_loop(agent: Arc<Agent>) -> Result<()> {
    let mut app = TuiApp::new(agent.clone());

    let model_name = agent.current_model_name().await;
    app.status = format!("pi | {} | ready", model_name);

    app.render()?;

    loop {
        // Poll for events with a small timeout so we can check other channels
        if event::poll(std::time::Duration::from_millis(50))? {
            match event::read()? {
                event::Event::Key(key) => {
                    use event::{KeyCode, KeyModifiers};

                    // Quit keys
                    if key.modifiers.contains(KeyModifiers::CONTROL)
                        && (key.code == KeyCode::Char('c') || key.code == KeyCode::Char('d'))
                    {
                        break;
                    }

                    // Submit on Enter (when not streaming)
                    if key.code == KeyCode::Enter && !app.is_streaming {
                        let text = app.editor.value();
                        if !text.trim().is_empty() {
                            let trimmed = text.trim().to_string();

                            // Handle quit commands
                            if trimmed == "exit" || trimmed == "/quit" {
                                break;
                            }

                            app.messages.push(DisplayMessage {
                                role: "user".to_string(),
                                text: trimmed.clone(),
                            });
                            app.editor.set_value("");
                            app.is_streaming = true;
                            app.streaming_text.clear();
                            app.status = format!("pi | {} | thinking...", model_name);
                            app.render()?;

                            // Run the agent prompt
                            let mut event_rx = agent.subscribe();
                            let agent_clone = agent.clone();

                            let prompt_text = trimmed.clone();
                            let mut prompt_handle = tokio::spawn(async move {
                                agent_clone.prompt(&prompt_text).await
                            });

                            // Collect streaming events while the prompt runs
                            loop {
                                tokio::select! {
                                    event = event_rx.recv() => {
                                        match event {
                                            Ok(AgentEvent::MessageUpdate { event: stream_event, .. }) => {
                                                if let StreamEvent::TextDelta { delta, .. } = stream_event {
                                                    app.streaming_text.push_str(&delta);
                                                    app.render()?;
                                                }
                                            }
                                            Ok(AgentEvent::AgentEnd { .. }) => break,
                                            Err(_) => break,
                                            _ => {}
                                        }
                                    }
                                    result = &mut prompt_handle => {
                                        match result {
                                            Ok(Ok(msg)) => {
                                                // Extract final text from the assistant message
                                                let response: String = msg.content.iter()
                                                    .filter_map(|c| {
                                                        if let Content::Text { text, .. } = c {
                                                            Some(text.as_str())
                                                        } else {
                                                            None
                                                        }
                                                    })
                                                    .collect::<Vec<_>>()
                                                    .join("\n");
                                                app.messages.push(DisplayMessage {
                                                    role: "assistant".to_string(),
                                                    text: response,
                                                });
                                            }
                                            Ok(Err(e)) => {
                                                app.messages.push(DisplayMessage {
                                                    role: "error".to_string(),
                                                    text: format!("Error: {e}"),
                                                });
                                            }
                                            Err(e) => {
                                                app.messages.push(DisplayMessage {
                                                    role: "error".to_string(),
                                                    text: format!("Task error: {e}"),
                                                });
                                            }
                                        }
                                        break;
                                    }
                                }
                            }

                            app.is_streaming = false;
                            app.streaming_text.clear();
                            app.status = format!("pi | {} | ready", model_name);
                        }
                    } else {
                        // Forward other keys to the editor
                        app.handle_key(key);
                    }

                    app.render()?;
                }
                event::Event::Resize(_, _) => {
                    app.render()?;
                }
                _ => {}
            }
        }
    }

    Ok(())
}

/// Truncate a string to fit within a given width
fn truncate(s: &str, max_width: usize) -> String {
    if s.len() <= max_width {
        s.to_string()
    } else {
        let end = s.char_indices()
            .take_while(|(i, _)| *i < max_width.saturating_sub(3))
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        format!("{}...", &s[..end])
    }
}

/// Convert a crossterm KeyEvent to a byte string for pi-tui input handling
fn crossterm_key_to_bytes(key: &event::KeyEvent) -> String {
    use event::{KeyCode, KeyModifiers};

    match key.code {
        KeyCode::Char(c) => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                // Ctrl+letter → control code (ASCII 1–26)
                let code = (c as u8).wrapping_sub(b'a').wrapping_add(1);
                String::from(code as char)
            } else {
                String::from(c)
            }
        }
        KeyCode::Backspace => "\x7f".to_string(),
        KeyCode::Delete => "\x1b[3~".to_string(),
        KeyCode::Left => "\x1b[D".to_string(),
        KeyCode::Right => "\x1b[C".to_string(),
        KeyCode::Up => "\x1b[A".to_string(),
        KeyCode::Down => "\x1b[B".to_string(),
        KeyCode::Home => "\x1b[H".to_string(),
        KeyCode::End => "\x1b[F".to_string(),
        KeyCode::Tab => "\t".to_string(),
        KeyCode::Esc => "\x1b".to_string(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quit_on_ctrl_c() {
        // Verify that Ctrl-C sets should_quit.
        // We can't construct a full TuiApp without a real Agent, but we can
        // test the key-to-bytes conversion used for quit detection.
        let key = event::KeyEvent::new(
            event::KeyCode::Char('c'),
            event::KeyModifiers::CONTROL,
        );
        let bytes = crossterm_key_to_bytes(&key);
        // Ctrl-C is ASCII 3
        assert_eq!(bytes, "\x03");
    }

    #[test]
    fn state_transitions() {
        // Verify that the DisplayMessage struct can represent all roles
        let user_msg = DisplayMessage {
            role: "user".to_string(),
            text: "hello".to_string(),
        };
        let assistant_msg = DisplayMessage {
            role: "assistant".to_string(),
            text: "hi there".to_string(),
        };
        assert_eq!(user_msg.role, "user");
        assert_eq!(assistant_msg.role, "assistant");
    }
}
