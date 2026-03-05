/// Full TUI interactive mode using pi-tui components.
///
/// This mode is now the default interactive path, aligned with pi-mono's
/// high-level behavior.
use std::collections::HashSet;
use std::io::{self, Write};
use std::path::Path;
use std::sync::{Arc, RwLock};

use anyhow::Result;
use crossterm::{
    cursor, event,
    terminal::{self, ClearType},
    ExecutableCommand,
};
use pi_agent_core::{Agent, AgentError, AgentEvent};
use pi_ai::{Content, Message, Model, StreamEvent, ThinkingLevel};
use pi_tui::components::markdown::MarkdownTheme;
use pi_tui::{
    Component, Editor, Focusable, Markdown, ModelInfo, ModelSelector,
    ThinkingLevel as TuiThinkingLevel, ThinkingSelector,
};

use super::interactive::{
    create_provider, detect_provider_from_key, get_default_model_for_provider,
    is_known_provider_name, mask_secret, provider_api_for_name, providers_help_text,
};

/// Application state for the TUI mode
struct TuiApp {
    editor: Editor,
    messages: Vec<DisplayMessage>,
    status: String,
    footer_provider: String,
    footer_model: String,
    footer_thinking: String,
    footer_auth_required: bool,
    tool_runs: u64,
    tool_errors: u64,
    turns: u64,
    streaming_text: String,
    is_streaming: bool,
}

/// A rendered message in the history
struct DisplayMessage {
    role: String,
    text: String,
}

enum CommandResult {
    NotACommand,
    Handled,
    Quit,
}

impl TuiApp {
    fn new() -> Self {
        let mut editor = Editor::new(3);
        editor.set_focused(true);

        Self {
            editor,
            messages: Vec::new(),
            status: String::new(),
            footer_provider: "n/a".to_string(),
            footer_model: "n/a".to_string(),
            footer_thinking: "off".to_string(),
            footer_auth_required: true,
            tool_runs: 0,
            tool_errors: 0,
            turns: 0,
            streaming_text: String::new(),
            is_streaming: false,
        }
    }

    fn push_message(&mut self, role: &str, text: impl Into<String>) {
        if role == "user" {
            self.turns += 1;
        }
        self.messages.push(DisplayMessage {
            role: role.to_string(),
            text: text.into(),
        });
    }

    fn push_system(&mut self, text: impl Into<String>) {
        self.push_message("system", text);
    }

    fn push_error(&mut self, text: impl Into<String>) {
        self.push_message("error", text);
    }

    fn push_warning(&mut self, text: impl Into<String>) {
        self.push_message("warning", text);
    }

    fn push_tool_start(&mut self, tool_name: &str) {
        self.tool_runs += 1;
        self.push_message("tool", format!("▶ {}  running", tool_name));
    }

    fn push_tool_end(&mut self, tool_name: &str, is_error: bool) {
        if is_error {
            self.tool_errors += 1;
        }
        let status = if is_error { "error" } else { "ok" };
        self.push_message("tool", format!("■ {}  {}", tool_name, status));
    }

    /// Render the entire screen
    fn render(&self) -> Result<()> {
        let mut stdout = io::stdout();
        let (cols, rows) = terminal::size()?;

        // Move to top-left and clear
        stdout.execute(cursor::MoveTo(0, 0))?;
        stdout.execute(terminal::Clear(ClearType::All))?;

        let header_h = 2u16;
        let footer_h = 1u16;
        let editor_inner_h = self.editor.height();
        let editor_box_h = editor_inner_h.saturating_add(2);
        let reserved = header_h
            .saturating_add(footer_h)
            .saturating_add(editor_box_h)
            .saturating_add(1);

        if cols < 36 || rows <= reserved {
            self.render_compact(cols, rows, &mut stdout)?;
            stdout.flush()?;
            return Ok(());
        }

        let chat_box_h = rows.saturating_sub(header_h + editor_box_h + footer_h);
        if chat_box_h < 3 {
            self.render_compact(cols, rows, &mut stdout)?;
            stdout.flush()?;
            return Ok(());
        }

        let chat_y = header_h;
        let editor_y = chat_y + chat_box_h;
        let footer_y = rows.saturating_sub(1);

        // Header
        let title = format!(
            "{} {} {}",
            style_accent("pi"),
            style_dim("interactive mode"),
            style_dim("(/quit to exit)")
        );
        let hints = format!(
            "{} {}",
            style_dim("keys:"),
            style_dim("Ctrl+L model | Ctrl+P/Shift+Ctrl+P cycle | Shift+Tab thinking | /model /thinking /setkey")
        );
        draw_row(&mut stdout, 0, cols, &title)?;
        draw_row(&mut stdout, 1, cols, &hints)?;

        // Conversation box
        draw_row(
            &mut stdout,
            chat_y,
            cols,
            &make_box_top(cols as usize, "Conversation"),
        )?;
        let chat_inner_h = chat_box_h.saturating_sub(2);
        let chat_inner_w = cols.saturating_sub(2) as usize;
        let mut history = self.render_history_lines(chat_inner_w);
        if history.len() > chat_inner_h as usize {
            history = history[history.len().saturating_sub(chat_inner_h as usize)..].to_vec();
        }
        for i in 0..chat_inner_h {
            let idx = i as usize;
            let line = history.get(idx).cloned().unwrap_or_default();
            let inside = format!(
                "│{}│",
                pad_or_truncate_visible(&style_chat_line(&line), chat_inner_w)
            );
            draw_row(&mut stdout, chat_y + 1 + i, cols, &inside)?;
        }
        draw_row(
            &mut stdout,
            chat_y + chat_box_h.saturating_sub(1),
            cols,
            &make_box_bottom(cols as usize),
        )?;

        // Prompt box
        draw_row(
            &mut stdout,
            editor_y,
            cols,
            &make_box_top(cols as usize, "Prompt"),
        )?;
        let editor_w = cols.saturating_sub(2);
        let editor_lines = self.editor.render(editor_w);
        for i in 0..editor_inner_h {
            let line = editor_lines.get(i as usize).cloned().unwrap_or_default();
            let inside = format!("│{}│", pad_or_truncate_visible(&line, editor_w as usize));
            draw_row(&mut stdout, editor_y + 1 + i, cols, &inside)?;
        }
        draw_row(
            &mut stdout,
            editor_y + editor_box_h.saturating_sub(1),
            cols,
            &make_box_bottom(cols as usize),
        )?;

        // Footer
        let footer = self.render_footer_row();
        draw_row(&mut stdout, footer_y, cols, &footer)?;

        stdout.flush()?;
        Ok(())
    }

    fn render_compact(&self, cols: u16, rows: u16, stdout: &mut io::Stdout) -> Result<()> {
        let mut row = 0u16;
        let history_max_row = rows.saturating_sub(6);
        let mut history = self.render_history_lines(cols as usize);
        if history.len() > history_max_row as usize {
            history = history[history.len().saturating_sub(history_max_row as usize)..].to_vec();
        }
        for line in history {
            if row >= history_max_row {
                break;
            }
            draw_row(stdout, row, cols, &style_chat_line(&line))?;
            row += 1;
        }

        let status_row = rows.saturating_sub(4);
        draw_row(stdout, status_row, cols, &self.render_footer_row())?;

        let editor_row = rows.saturating_sub(3);
        let rendered = self.editor.render(cols);
        for (i, line) in rendered.iter().enumerate() {
            let y = editor_row + i as u16;
            if y < rows {
                draw_row(stdout, y, cols, line)?;
            }
        }
        Ok(())
    }

    fn render_history_lines(&self, width: usize) -> Vec<String> {
        let mut out = Vec::new();
        for msg in &self.messages {
            let (lead, cont) = role_prefix(&msg.role);
            push_wrapped_message_lines(&mut out, &msg.role, lead, cont, &msg.text, width);
        }

        if self.is_streaming && !self.streaming_text.is_empty() {
            let (lead, cont) = role_prefix("assistant");
            push_wrapped_message_lines(
                &mut out,
                "assistant_streaming",
                lead,
                cont,
                &self.streaming_text,
                width,
            );
        }

        out
    }

    fn render_footer_row(&self) -> String {
        let user_count = self.messages.iter().filter(|m| m.role == "user").count();
        let assistant_count = self
            .messages
            .iter()
            .filter(|m| m.role == "assistant")
            .count();
        let session = format!(
            "msg:{} turn:{} u:{} a:{}",
            self.messages.len(),
            self.turns,
            user_count,
            assistant_count
        );
        let tools = format!("{}/{}", self.tool_runs, self.tool_errors);
        format!(
            "{} {}  {} {}  {} {}  {} {}  {} {}  {} {}",
            style_dim("status"),
            style_status(&self.status),
            style_dim("provider"),
            style_accent(&self.footer_provider),
            style_dim("model"),
            style_accent(&self.footer_model),
            style_dim("thinking"),
            style_accent(&self.footer_thinking),
            style_dim("tools"),
            if self.tool_errors > 0 {
                style_warning(&tools)
            } else {
                style_accent(&tools)
            },
            style_dim("session"),
            style_dim(&session),
        )
    }

    /// Handle a crossterm key event
    fn handle_key(&mut self, key: event::KeyEvent) {
        // Convert crossterm key event to a byte string for pi-tui input handling
        let data = crossterm_key_to_bytes(&key);
        if !data.is_empty() {
            self.editor.handle_input(&data);
        }
    }
}

/// Run the full TUI interactive mode.
pub async fn run_tui_mode(
    agent: Arc<Agent>,
    runtime_api_key: Arc<RwLock<Option<String>>>,
) -> Result<()> {
    // Enter raw mode and alternate screen
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    stdout.execute(terminal::EnterAlternateScreen)?;
    stdout.execute(cursor::Hide)?;

    let result = run_tui_loop(agent, runtime_api_key).await;

    // Cleanup
    let mut stdout = io::stdout();
    stdout.execute(cursor::Show)?;
    stdout.execute(terminal::LeaveAlternateScreen)?;
    terminal::disable_raw_mode()?;

    result
}

async fn run_tui_loop(
    agent: Arc<Agent>,
    runtime_api_key: Arc<RwLock<Option<String>>>,
) -> Result<()> {
    let mut app = TuiApp::new();
    let mut catalog = crate::skills::SkillCatalog::discover(Path::new(&agent.config.cwd))?;
    let mut active_skills = crate::skills::ActiveSkills::default();

    app.push_system("session started");
    app.push_system("hint: /model /thinking /providers /skills");
    app.push_system("hint: /setkey <api-key> to configure runtime credentials");
    if !catalog.is_empty() {
        app.push_system(format!(
            "skills loaded: {} (use /skills, /skill:list, /skill:<name>, /skill:clear, /skill:install <path>)",
            catalog.len()
        ));
    }
    set_ready_status(&mut app, &agent).await;
    if app.footer_auth_required {
        app.push_warning("auth required: no provider credentials detected. Use /setkey <api-key>.");
    }
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

                    if !app.is_streaming {
                        let is_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                        let is_shift = key.modifiers.contains(KeyModifiers::SHIFT);
                        let key_char = match key.code {
                            KeyCode::Char(c) => Some(c.to_ascii_lowercase()),
                            _ => None,
                        };

                        // Cycle thinking level (Shift+Tab)
                        if key.code == KeyCode::BackTab || (key.code == KeyCode::Tab && is_shift) {
                            let current_model = agent.get_current_model().await;
                            if !current_model.supports_reasoning() {
                                app.push_system(
                                    "[thinking] current model does not support thinking",
                                );
                            } else {
                                let next = cycle_thinking_level(agent.get_thinking_level());
                                agent.update_thinking_level(next);
                                app.push_system(format!(
                                    "[thinking] level: {}",
                                    format_thinking_level(next)
                                ));
                            }
                            set_ready_status(&mut app, &agent).await;
                            app.render()?;
                            continue;
                        }

                        // Cycle model forward/backward (Ctrl+P / Shift+Ctrl+P)
                        if is_ctrl && key_char == Some('p') {
                            let switched = if is_shift {
                                agent.cycle_model_prev().await
                            } else {
                                agent.cycle_model_next().await
                            };
                            if switched.is_some() {
                                let model = agent.get_current_model().await;
                                let thinking = agent.get_thinking_level();
                                app.push_system(format!(
                                    "[model] switched to {}{}",
                                    model.id,
                                    render_thinking_suffix(&model, thinking)
                                ));
                            } else {
                                app.push_system("[model] only one model available");
                            }
                            set_ready_status(&mut app, &agent).await;
                            app.render()?;
                            continue;
                        }

                        // Open model selector (Ctrl+L)
                        if is_ctrl && key_char == Some('l') {
                            open_model_selector(&mut app, &agent, None).await?;
                            set_ready_status(&mut app, &agent).await;
                            app.render()?;
                            continue;
                        }

                        // Slash command completion (Tab) when not streaming.
                        if key.code == KeyCode::Tab && key.modifiers.is_empty() {
                            if apply_slash_autocomplete(&mut app, &catalog) {
                                app.render()?;
                                continue;
                            }
                        }
                    }

                    // Submit on Enter (when not streaming)
                    if key.code == KeyCode::Enter && !app.is_streaming {
                        let text = app.editor.value();
                        let trimmed = text.trim().to_string();
                        if trimmed.is_empty() {
                            app.render()?;
                            continue;
                        }

                        match handle_command(
                            &trimmed,
                            &agent,
                            &runtime_api_key,
                            &mut catalog,
                            &mut active_skills,
                            &mut app,
                        )
                        .await?
                        {
                            CommandResult::Quit => break,
                            CommandResult::Handled => {
                                app.editor.set_value("");
                                app.render()?;
                                continue;
                            }
                            CommandResult::NotACommand => {}
                        }

                        app.push_message("user", trimmed.clone());
                        app.editor.set_value("");

                        let processed = match crate::input::file_processor::process_input(
                            &trimmed,
                            Path::new(&agent.config.cwd),
                        ) {
                            Ok(v) => v,
                            Err(e) => {
                                app.push_error(format!("Failed to process input: {e}"));
                                set_ready_status(&mut app, &agent).await;
                                app.render()?;
                                continue;
                            }
                        };
                        let prompt_text = crate::skills::decorate_user_text(
                            &processed.text,
                            &catalog,
                            &active_skills,
                        );
                        let mut blocks = Vec::new();
                        if !prompt_text.is_empty() {
                            blocks.push(Content::text(prompt_text));
                        }
                        blocks.extend(processed.images.iter().map(|img| img.to_content()));

                        if blocks.is_empty() {
                            set_ready_status(&mut app, &agent).await;
                            app.render()?;
                            continue;
                        }

                        app.is_streaming = true;
                        app.streaming_text.clear();
                        app.status = "thinking...".to_string();
                        app.render()?;

                        // Run the agent prompt
                        let mut event_rx = agent.subscribe();
                        let agent_clone = agent.clone();
                        let input = Message::user_with_images(blocks);
                        let mut prompt_handle =
                            tokio::spawn(async move { agent_clone.prompt_message(input).await });

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
                                        Ok(AgentEvent::ToolExecutionStart { tool_name, .. }) => {
                                            app.push_tool_start(&tool_name);
                                            app.render()?;
                                        }
                                        Ok(AgentEvent::ToolExecutionEnd { tool_name, is_error, .. }) => {
                                            app.push_tool_end(&tool_name, is_error);
                                            app.render()?;
                                        }
                                        Ok(AgentEvent::ToolApprovalRequired {
                                            call_id,
                                            tool_name,
                                            arguments,
                                        }) => {
                                            handle_tool_approval_request(
                                                &mut app,
                                                &agent,
                                                call_id,
                                                tool_name,
                                                arguments.to_string(),
                                            )
                                            .await?;
                                            app.status = "thinking...".to_string();
                                            app.render()?;
                                        }
                                        Ok(AgentEvent::AgentEnd { .. }) => {
                                            // Wait for prompt task result to ensure final message is captured.
                                        }
                                        Err(_) => {}
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
                                            app.push_message("assistant", response);
                                        }
                                        Ok(Err(e)) => {
                                            match e {
                                                AgentError::NoProvider => {
                                                    app.push_error("Error: No provider configured");
                                                    app.push_system(
                                                        "[auth] use /setkey <api-key> to configure a runtime provider",
                                                    );
                                                }
                                                other => {
                                                    app.push_error(format!("Error: {other}"));
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            app.push_error(format!("Task error: {e}"));
                                        }
                                    }
                                    break;
                                }
                            }
                        }

                        app.is_streaming = false;
                        app.streaming_text.clear();
                        set_ready_status(&mut app, &agent).await;
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

async fn set_ready_status(app: &mut TuiApp, agent: &Agent) {
    let model = agent.get_current_model().await;
    let has_provider = pi_ai::get_provider(&model.api.to_string()).is_some();
    app.footer_provider = model.provider.to_string();
    app.footer_model = model.id.clone();
    app.footer_thinking = if model.supports_reasoning() {
        format_thinking_level(agent.get_thinking_level()).to_string()
    } else {
        "n/a".to_string()
    };
    app.footer_auth_required = !has_provider;
    app.status = if has_provider {
        "ready".to_string()
    } else {
        "auth required".to_string()
    };
}

async fn handle_tool_approval_request(
    app: &mut TuiApp,
    agent: &Agent,
    call_id: String,
    tool_name: String,
    arguments: String,
) -> Result<()> {
    app.push_message(
        "tool",
        format!(
            "? approval needed: {} ({})",
            tool_name,
            truncate(&arguments, 120)
        ),
    );
    app.push_warning("approval: press y to allow, n/Esc to deny");
    app.status = "approval required".to_string();
    app.render()?;

    loop {
        if event::poll(std::time::Duration::from_millis(100))? {
            match event::read()? {
                event::Event::Key(key) => {
                    use event::KeyCode;

                    match key.code {
                        KeyCode::Char('y') | KeyCode::Char('Y') => {
                            agent.approve_tool(&call_id, true).await;
                            app.push_message("tool", "✔ approval granted");
                            return Ok(());
                        }
                        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                            agent.approve_tool(&call_id, false).await;
                            app.push_message("tool", "✖ approval denied");
                            return Ok(());
                        }
                        _ => {}
                    }
                }
                event::Event::Resize(_, _) => {
                    app.render()?;
                }
                _ => {}
            }
        }
    }
}

async fn handle_command(
    input: &str,
    agent: &Arc<Agent>,
    runtime_api_key: &Arc<RwLock<Option<String>>>,
    catalog: &mut crate::skills::SkillCatalog,
    active_skills: &mut crate::skills::ActiveSkills,
    app: &mut TuiApp,
) -> Result<CommandResult> {
    if input == "exit" || input == "/quit" {
        return Ok(CommandResult::Quit);
    }

    if input == "/setkey" || input == "/apikey" {
        app.push_system("[auth] usage: /setkey <api-key> (or /setkey clear)");
        return Ok(CommandResult::Handled);
    }

    if let Some(raw) = input
        .strip_prefix("/setkey ")
        .or_else(|| input.strip_prefix("/apikey "))
    {
        let value = raw.trim();
        if value.is_empty() {
            app.push_system("[auth] usage: /setkey <api-key> (or /setkey clear)");
            return Ok(CommandResult::Handled);
        }

        if value.eq_ignore_ascii_case("clear") {
            *runtime_api_key.write().unwrap_or_else(|e| e.into_inner()) = None;
            if let Err(err) = crate::auth::clear_persisted_auth() {
                app.push_error(format!(
                    "[auth] warning: failed to clear persisted credentials: {err}"
                ));
            } else {
                app.push_system("[auth] persisted credentials cleared");
            }
            app.push_system("[auth] runtime API key cleared (falling back to provider defaults)");
            return Ok(CommandResult::Handled);
        }

        *runtime_api_key.write().unwrap_or_else(|e| e.into_inner()) = Some(value.to_string());
        app.push_system(format!(
            "[auth] runtime API key set: {}",
            mask_secret(value)
        ));

        // Try to detect provider from key format and re-register it.
        let detected = detect_provider_from_key(value);
        let persisted_provider = if is_known_provider_name(detected) {
            Some(detected)
        } else {
            None
        };
        if let Err(err) = crate::auth::save_persisted_auth(value, persisted_provider) {
            app.push_error(format!(
                "[auth] warning: failed to persist credentials: {err}"
            ));
        } else {
            app.push_system("[auth] credentials persisted to ~/.pi/agent/auth.json");
        }
        if detected != "unknown" {
            match create_provider(detected, value) {
                Ok(new_provider) => {
                    use pi_ai::messages::types::Api;
                    let api_key = match detected {
                        "anthropic" => Api::AnthropicMessages.to_string(),
                        "openai" => Api::OpenAICompletions.to_string(),
                        "google" => Api::GoogleGenerativeAI.to_string(),
                        "groq" | "openrouter" => Api::OpenAICompletions.to_string(),
                        _ => Api::OpenAICompletions.to_string(),
                    };
                    pi_ai::register_provider(&api_key, new_provider.clone());

                    let provider_api = provider_api_for_name(detected);
                    agent.update_provider_api(provider_api);
                    let default_model = get_default_model_for_provider(detected);
                    let model_id = default_model.id.clone();
                    agent.update_model(default_model).await;
                    app.push_system(format!(
                        "[auth] provider '{}' activated with model '{}'",
                        detected, model_id
                    ));
                    app.push_system("[auth] you can now send messages without restarting");
                }
                Err(e) => {
                    app.push_error(format!("[auth] warning: failed to create provider: {e}"));
                    app.push_system(format!(
                        "[auth] detected provider: {detected}. You may need to restart with --provider {detected}"
                    ));
                }
            }
        } else {
            app.push_system(format!(
                "[auth] detected provider: unknown (try anthropic, openai, google, groq, openrouter). Restart with --provider <name> to use it."
            ));
        }

        set_ready_status(app, agent).await;
        return Ok(CommandResult::Handled);
    }

    if input == "/providers" {
        app.push_system(providers_help_text());
        return Ok(CommandResult::Handled);
    }

    if input == "/provider" {
        let current = agent.get_current_model().await;
        app.push_system(format!(
            "[provider] usage: /provider <name> (or /providers to list)\n[provider] current: {} (model: {})",
            current.provider, current.id
        ));
        return Ok(CommandResult::Handled);
    }

    if let Some(name) = input.strip_prefix("/provider ") {
        let provider_name = name.trim();
        if provider_name.is_empty() {
            app.push_system("[provider] usage: /provider <name> (or /providers to list)");
        } else {
            app.push_system(format!(
                "[provider] to switch to '{}', restart with: pi --provider {}",
                provider_name, provider_name
            ));
        }
        return Ok(CommandResult::Handled);
    }

    if input == "/model" {
        open_model_selector(app, agent, None).await?;
        set_ready_status(app, agent).await;
        return Ok(CommandResult::Handled);
    }

    if let Some(raw) = input.strip_prefix("/model ") {
        let query = raw.trim();
        if query.is_empty() {
            app.push_system("[model] usage: /model <id-or-search> (or /model)");
            return Ok(CommandResult::Handled);
        }

        let candidates = model_candidates();
        match find_exact_model_match(&candidates, query) {
            Some(model) => {
                switch_to_model(agent, app, model).await;
                set_ready_status(app, agent).await;
            }
            None => {
                open_model_selector(app, agent, Some(query)).await?;
                set_ready_status(app, agent).await;
            }
        }
        return Ok(CommandResult::Handled);
    }

    if input == "/thinking" {
        open_thinking_selector(app, agent).await?;
        set_ready_status(app, agent).await;
        return Ok(CommandResult::Handled);
    }

    if let Some(raw) = input.strip_prefix("/thinking ") {
        let value = raw.trim();
        let Some(parsed) = parse_thinking_level(value) else {
            app.push_system("[thinking] usage: /thinking <off|minimal|low|medium|high|xhigh>");
            return Ok(CommandResult::Handled);
        };

        let current = agent.get_current_model().await;
        if parsed.is_some() && !current.supports_reasoning() {
            app.push_system("[thinking] current model does not support thinking");
            return Ok(CommandResult::Handled);
        }

        agent.update_thinking_level(parsed);
        app.push_system(format!(
            "[thinking] level: {}",
            format_thinking_level(parsed)
        ));
        set_ready_status(app, agent).await;
        return Ok(CommandResult::Handled);
    }

    if input == "/skills" || input == "/skill:list" {
        app.push_system(render_skill_list(catalog, active_skills));
        return Ok(CommandResult::Handled);
    }

    if input == "/skill:clear" {
        active_skills.clear();
        app.push_system("[skills] cleared");
        return Ok(CommandResult::Handled);
    }

    if let Some(path) = input.strip_prefix("/skill:install ") {
        let source = Path::new(path.trim());
        match crate::skills::install_skill_into_project(Path::new(&agent.config.cwd), source) {
            Ok(installed) => {
                crate::skills::register_skill_tool(agent, installed.clone()).await;
                catalog.upsert(installed.clone());
                app.push_system(format!(
                    "[skills] installed '{}' at {}",
                    installed.name,
                    installed.path.display()
                ));
            }
            Err(err) => {
                app.push_error(format!("[skills] install failed: {err}"));
            }
        }
        return Ok(CommandResult::Handled);
    }

    if let Some(name) = input.strip_prefix("/skill:") {
        if name.trim().is_empty() {
            app.push_system("[skills] usage: /skill:<name> (or /skill:list)");
            return Ok(CommandResult::Handled);
        }
        if let Some(skill) = catalog.get(name.trim()) {
            active_skills.set(&skill.name);
            app.push_system(format!("[skills] activated '{}'", skill.name));
        } else {
            app.push_error(format!("[skills] '{}' not found", name.trim()));
        }
        return Ok(CommandResult::Handled);
    }

    if input.starts_with('/') {
        app.push_error(format!(
            "[command] unknown slash command: {input} (try /model, /thinking, /providers, or /skill:list)"
        ));
        return Ok(CommandResult::Handled);
    }

    Ok(CommandResult::NotACommand)
}

fn render_skill_list(
    catalog: &crate::skills::SkillCatalog,
    active: &crate::skills::ActiveSkills,
) -> String {
    if catalog.is_empty() {
        return "[skills] none found under ~/.pi/skills or .pi/skills".to_string();
    }

    let mut lines = Vec::new();
    let active_names = active.list();
    for name in catalog.names() {
        let marker = if active_names.contains(&name) {
            "*"
        } else {
            " "
        };
        if let Some(skill) = catalog.get(&name) {
            lines.push(format!(
                "[{}] {} - {} ({})",
                marker,
                skill.name,
                skill.description,
                skill.path.display()
            ));
        }
    }
    lines.join("\n")
}

fn apply_slash_autocomplete(app: &mut TuiApp, catalog: &crate::skills::SkillCatalog) -> bool {
    let current = app.editor.value();
    if current.contains('\n') {
        return false;
    }

    let trimmed_start = current.trim_start();
    let leading_ws_len = current.len().saturating_sub(trimmed_start.len());
    let leading_ws = &current[..leading_ws_len];
    let mut candidates = slash_completion_candidates(trimmed_start, catalog);
    if candidates.is_empty() {
        return false;
    }

    candidates.sort_by_key(|v| v.to_ascii_lowercase());
    candidates.dedup_by(|a, b| a.eq_ignore_ascii_case(b));

    if candidates.len() == 1 {
        let completed = format!("{leading_ws}{}", candidates[0]);
        if completed != current {
            set_editor_value_at_end(&mut app.editor, &completed);
        }
        return true;
    }

    let common = longest_common_prefix(&candidates);
    if common.len() > trimmed_start.len() {
        let completed = format!("{leading_ws}{common}");
        set_editor_value_at_end(&mut app.editor, &completed);
        return true;
    }

    let preview = candidates
        .iter()
        .take(6)
        .cloned()
        .collect::<Vec<_>>()
        .join("  ");
    if !preview.is_empty() {
        let suffix = if candidates.len() > 6 { "  ..." } else { "" };
        app.push_system(format!("[suggest] {}{}", preview, suffix));
    }
    true
}

fn set_editor_value_at_end(editor: &mut Editor, value: &str) {
    editor.set_value(value);
    // Ctrl+E: move cursor to end of line after replacing the editor buffer.
    editor.handle_input("\x05");
}

fn slash_completion_candidates(
    partial: &str,
    catalog: &crate::skills::SkillCatalog,
) -> Vec<String> {
    if partial.is_empty() || !partial.starts_with('/') {
        return Vec::new();
    }

    let lower = partial.to_ascii_lowercase();
    let mut out = Vec::new();

    let static_commands = [
        "/quit",
        "/setkey ",
        "/setkey clear",
        "/apikey ",
        "/apikey clear",
        "/providers",
        "/provider ",
        "/model",
        "/model ",
        "/thinking",
        "/thinking ",
        "/skills",
        "/skill:list",
        "/skill:clear",
        "/skill:install ",
    ];
    for cmd in static_commands {
        if cmd.starts_with(&lower) {
            out.push(cmd.to_string());
        }
    }

    let providers = [
        "anthropic",
        "openai",
        "google",
        "groq",
        "openrouter",
        "azure",
        "bedrock",
    ];
    if let Some(arg) = lower.strip_prefix("/provider ") {
        for provider in providers {
            if provider.starts_with(arg) {
                out.push(format!("/provider {}", provider));
            }
        }
    }

    let thinking_levels = ["off", "minimal", "low", "medium", "high", "xhigh"];
    if let Some(arg) = lower.strip_prefix("/thinking ") {
        for level in thinking_levels {
            if level.starts_with(arg) {
                out.push(format!("/thinking {}", level));
            }
        }
    }

    if lower.starts_with("/skill:") {
        for name in catalog.names() {
            let candidate = format!("/skill:{}", name);
            if candidate.to_ascii_lowercase().starts_with(&lower) {
                out.push(candidate);
            }
        }
    }

    out
}

fn longest_common_prefix(values: &[String]) -> String {
    if values.is_empty() {
        return String::new();
    }

    let mut prefix = values[0].clone();
    for value in values.iter().skip(1) {
        let mut next = String::new();
        for (a, b) in prefix.chars().zip(value.chars()) {
            if a == b {
                next.push(a);
            } else {
                break;
            }
        }
        prefix = next;
        if prefix.is_empty() {
            break;
        }
    }

    prefix
}

async fn switch_to_model(agent: &Arc<Agent>, app: &mut TuiApp, model: Model) {
    let provider_api = model.api.to_string();
    if pi_ai::get_provider(&provider_api).is_none() {
        app.push_error(format!(
            "[model] provider '{}' is not configured. Set credentials and /setkey first.",
            provider_api
        ));
        return;
    }

    agent.update_provider_api(provider_api);
    agent.update_model(model.clone()).await;
    if !model.supports_reasoning() {
        agent.update_thinking_level(None);
    }
    app.push_system(format!(
        "[model] switched to {}{}",
        model.id,
        render_thinking_suffix(&model, agent.get_thinking_level())
    ));
}

fn model_to_info(model: &Model) -> ModelInfo {
    let mut capabilities = Vec::new();
    if model.supports_reasoning() {
        capabilities.push("reasoning".to_string());
    }
    if model.supports_images() {
        capabilities.push("vision".to_string());
    }

    let mut info = ModelInfo::new(
        model_selector_id(model),
        model.name.clone(),
        model.provider.to_string(),
    );

    if let Ok(window) = usize::try_from(model.context_window) {
        info = info.with_context_window(window);
    }
    if model.cost.input > 0.0 || model.cost.output > 0.0 {
        info = info.with_costs(model.cost.input, model.cost.output);
    }
    info.with_capabilities(capabilities)
}

fn filter_models(models: &[Model], query: &str) -> Vec<Model> {
    if query.trim().is_empty() {
        return models.to_vec();
    }

    let q = query.to_ascii_lowercase();
    models
        .iter()
        .filter(|m| {
            m.id.to_ascii_lowercase().contains(&q)
                || m.name.to_ascii_lowercase().contains(&q)
                || m.provider.to_string().to_ascii_lowercase().contains(&q)
                || format!("{}/{}", m.provider, m.id)
                    .to_ascii_lowercase()
                    .contains(&q)
        })
        .cloned()
        .collect()
}

fn model_selector_id(model: &Model) -> String {
    format!("{}/{}", model.provider, model.id)
}

fn model_candidates() -> Vec<Model> {
    let configured_apis: HashSet<String> = pi_ai::get_providers()
        .into_iter()
        .map(|(api, _)| api)
        .collect();

    let mut models: Vec<Model> = if configured_apis.is_empty() {
        Vec::new()
    } else {
        pi_ai::built_in_models()
            .iter()
            .filter(|m| configured_apis.contains(&m.api.to_string()))
            .cloned()
            .collect()
    };

    if models.is_empty() {
        models = pi_ai::built_in_models().to_vec();
    }

    models.sort_by(|a, b| {
        a.provider
            .to_string()
            .cmp(&b.provider.to_string())
            .then_with(|| a.id.cmp(&b.id))
    });
    models.dedup_by(|a, b| a.provider == b.provider && a.id == b.id);
    models
}

fn find_exact_model_match(models: &[Model], query: &str) -> Option<Model> {
    let term = query.trim();
    if term.is_empty() {
        return None;
    }

    let (target_provider, target_model) = if let Some((provider, model_id)) = term.split_once('/') {
        (
            Some(provider.trim().to_ascii_lowercase()),
            model_id.trim().to_ascii_lowercase(),
        )
    } else {
        (None, term.to_ascii_lowercase())
    };

    if target_model.is_empty() {
        return None;
    }

    let matches: Vec<Model> = models
        .iter()
        .filter(|m| {
            let id_match = m.id.to_ascii_lowercase() == target_model;
            let provider_match = match target_provider.as_ref() {
                Some(provider) => m.provider.to_string().to_ascii_lowercase() == *provider,
                None => true,
            };
            id_match && provider_match
        })
        .cloned()
        .collect();

    if matches.len() == 1 {
        matches.into_iter().next()
    } else {
        None
    }
}

fn render_model_selector_overlay(
    app: &TuiApp,
    selector: &ModelSelector,
    query: &str,
    shown: usize,
    total: usize,
) -> Result<()> {
    app.render()?;

    let mut stdout = io::stdout();
    let (cols, rows) = terminal::size()?;
    let width = cols.saturating_sub(4).max(20);
    let max_lines = rows.saturating_sub(8) as usize;

    let mut lines = selector.render(width);
    lines.insert(
        0,
        "\x1b[1mSelect Model\x1b[0m (type to filter, Backspace clear, ↑↓ move, Enter confirm, Esc cancel)".to_string(),
    );
    lines.insert(
        1,
        format!(
            "Filter: '{}'  [{shown}/{total}]",
            if query.is_empty() { "" } else { query }
        ),
    );
    if lines.len() > max_lines {
        lines.truncate(max_lines);
    }

    for (i, line) in lines.iter().enumerate() {
        let y = 1 + i as u16;
        if y >= rows.saturating_sub(4) {
            break;
        }
        stdout.execute(cursor::MoveTo(2, y))?;
        stdout.execute(terminal::Clear(ClearType::CurrentLine))?;
        write!(stdout, "{}", truncate(line, width as usize))?;
    }
    stdout.flush()?;
    Ok(())
}

async fn open_model_selector(
    app: &mut TuiApp,
    agent: &Arc<Agent>,
    initial_query: Option<&str>,
) -> Result<()> {
    let current = agent.get_current_model().await;
    let all_models = model_candidates();

    if all_models.is_empty() {
        app.push_system("[model] no models available");
        return Ok(());
    }

    let mut query = initial_query.unwrap_or("").trim().to_string();
    let mut selected_id = model_selector_id(&current);

    loop {
        let filtered = filter_models(&all_models, &query);
        let infos = filtered.iter().map(model_to_info).collect::<Vec<_>>();
        let mut selector = ModelSelector::new(infos);
        selector.set_focused(true);
        selector.set_selected(&selected_id);

        render_model_selector_overlay(app, &selector, &query, filtered.len(), all_models.len())?;
        if event::poll(std::time::Duration::from_millis(100))? {
            match event::read()? {
                event::Event::Key(key) => {
                    use event::{KeyCode, KeyModifiers};

                    if key.modifiers.contains(KeyModifiers::CONTROL)
                        && (key.code == KeyCode::Char('c') || key.code == KeyCode::Char('d'))
                    {
                        app.push_system("[model] selection cancelled");
                        return Ok(());
                    }

                    match key.code {
                        KeyCode::Esc => {
                            app.push_system("[model] selection cancelled");
                            return Ok(());
                        }
                        KeyCode::Enter => {
                            selector.confirm_selection();
                            if let Some(selected_id) = selector.selected_id() {
                                if let Some(model) = all_models
                                    .iter()
                                    .find(|m| model_selector_id(m) == selected_id)
                                    .cloned()
                                {
                                    switch_to_model(agent, app, model).await;
                                }
                            }
                            return Ok(());
                        }
                        KeyCode::Backspace => {
                            query.pop();
                        }
                        KeyCode::Char(c) => {
                            if !key.modifiers.contains(KeyModifiers::CONTROL)
                                && !key.modifiers.contains(KeyModifiers::ALT)
                            {
                                query.push(c);
                            } else {
                                let data = crossterm_key_to_bytes(&key);
                                if !data.is_empty() {
                                    selector.handle_input(&data);
                                    selector.confirm_selection();
                                    if let Some(id) = selector.selected_id() {
                                        selected_id = id.to_string();
                                    }
                                }
                            }
                        }
                        _ => {
                            let data = crossterm_key_to_bytes(&key);
                            if !data.is_empty() {
                                selector.handle_input(&data);
                                selector.confirm_selection();
                                if let Some(id) = selector.selected_id() {
                                    selected_id = id.to_string();
                                }
                            }
                        }
                    }
                }
                event::Event::Resize(_, _) => {}
                _ => {}
            }
        }
    }
}

fn thinking_levels_for_selector() -> Vec<TuiThinkingLevel> {
    vec![
        TuiThinkingLevel::new("off", "Off", "No extended thinking").with_color("gray"),
        TuiThinkingLevel::new("minimal", "Minimal", "Light reasoning effort")
            .with_budget(1_024)
            .with_color("green"),
        TuiThinkingLevel::new("low", "Low", "Low reasoning effort")
            .with_budget(4_096)
            .with_color("cyan"),
        TuiThinkingLevel::new("medium", "Medium", "Balanced reasoning effort")
            .with_budget(10_240)
            .with_color("yellow"),
        TuiThinkingLevel::new("high", "High", "High reasoning effort")
            .with_budget(32_768)
            .with_color("red"),
        TuiThinkingLevel::new("xhigh", "XHigh", "Maximum/provider-managed reasoning")
            .with_color("magenta"),
    ]
}

fn render_thinking_selector_overlay(app: &TuiApp, selector: &ThinkingSelector) -> Result<()> {
    app.render()?;

    let mut stdout = io::stdout();
    let (cols, rows) = terminal::size()?;
    let width = cols.saturating_sub(4).max(20);
    let max_lines = rows.saturating_sub(8) as usize;

    let mut lines = selector.render(width);
    lines.insert(
        0,
        "\x1b[1mSelect Thinking\x1b[0m (↑↓ move, Enter confirm, Esc cancel)".to_string(),
    );
    if lines.len() > max_lines {
        lines.truncate(max_lines);
    }

    for (i, line) in lines.iter().enumerate() {
        let y = 1 + i as u16;
        if y >= rows.saturating_sub(4) {
            break;
        }
        stdout.execute(cursor::MoveTo(2, y))?;
        stdout.execute(terminal::Clear(ClearType::CurrentLine))?;
        write!(stdout, "{}", truncate(line, width as usize))?;
    }
    stdout.flush()?;
    Ok(())
}

async fn open_thinking_selector(app: &mut TuiApp, agent: &Arc<Agent>) -> Result<()> {
    let current = agent.get_current_model().await;
    if !current.supports_reasoning() {
        app.push_system("[thinking] current model does not support thinking");
        return Ok(());
    }

    let mut selector = ThinkingSelector::with_levels(thinking_levels_for_selector());
    selector.set_focused(true);
    selector.set_selected(format_thinking_level(agent.get_thinking_level()));

    loop {
        render_thinking_selector_overlay(app, &selector)?;
        if event::poll(std::time::Duration::from_millis(100))? {
            match event::read()? {
                event::Event::Key(key) => {
                    use event::{KeyCode, KeyModifiers};

                    if key.modifiers.contains(KeyModifiers::CONTROL)
                        && (key.code == KeyCode::Char('c') || key.code == KeyCode::Char('d'))
                    {
                        app.push_system("[thinking] selection cancelled");
                        return Ok(());
                    }

                    match key.code {
                        KeyCode::Esc => {
                            app.push_system("[thinking] selection cancelled");
                            return Ok(());
                        }
                        KeyCode::Enter => {
                            selector.confirm_selection();
                            if let Some(id) = selector.selected_id() {
                                if let Some(parsed) = parse_thinking_level(id) {
                                    agent.update_thinking_level(parsed);
                                    app.push_system(format!(
                                        "[thinking] level: {}",
                                        format_thinking_level(parsed)
                                    ));
                                }
                            }
                            return Ok(());
                        }
                        _ => {
                            let data = crossterm_key_to_bytes(&key);
                            if !data.is_empty() {
                                selector.handle_input(&data);
                                selector.confirm_selection();
                            }
                        }
                    }
                }
                event::Event::Resize(_, _) => {}
                _ => {}
            }
        }
    }
}

fn parse_thinking_level(raw: &str) -> Option<Option<ThinkingLevel>> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "off" | "none" => Some(None),
        "minimal" => Some(Some(ThinkingLevel::Minimal)),
        "low" => Some(Some(ThinkingLevel::Low)),
        "medium" => Some(Some(ThinkingLevel::Medium)),
        "high" => Some(Some(ThinkingLevel::High)),
        "xhigh" => Some(Some(ThinkingLevel::XHigh)),
        _ => None,
    }
}

fn format_thinking_level(level: Option<ThinkingLevel>) -> &'static str {
    match level {
        None => "off",
        Some(ThinkingLevel::Minimal) => "minimal",
        Some(ThinkingLevel::Low) => "low",
        Some(ThinkingLevel::Medium) => "medium",
        Some(ThinkingLevel::High) => "high",
        Some(ThinkingLevel::XHigh) => "xhigh",
    }
}

fn cycle_thinking_level(current: Option<ThinkingLevel>) -> Option<ThinkingLevel> {
    match current {
        None => Some(ThinkingLevel::Minimal),
        Some(ThinkingLevel::Minimal) => Some(ThinkingLevel::Low),
        Some(ThinkingLevel::Low) => Some(ThinkingLevel::Medium),
        Some(ThinkingLevel::Medium) => Some(ThinkingLevel::High),
        Some(ThinkingLevel::High) => Some(ThinkingLevel::XHigh),
        Some(ThinkingLevel::XHigh) => None,
    }
}

fn render_thinking_suffix(model: &Model, thinking_level: Option<ThinkingLevel>) -> String {
    if model.supports_reasoning() {
        if thinking_level.is_some() {
            return format!(" (thinking: {})", format_thinking_level(thinking_level));
        }
    }
    String::new()
}

fn draw_row(stdout: &mut io::Stdout, y: u16, cols: u16, text: &str) -> Result<()> {
    stdout.execute(cursor::MoveTo(0, y))?;
    stdout.execute(terminal::Clear(ClearType::CurrentLine))?;
    write!(stdout, "{}", pad_or_truncate_visible(text, cols as usize))?;
    Ok(())
}

fn make_box_top(width: usize, title: &str) -> String {
    if width < 2 {
        return String::new();
    }
    let inner = width.saturating_sub(2);
    if inner == 0 {
        return "┌┐".to_string();
    }
    let centered = format!(" {title} ");
    let title_block = truncate(&centered, inner);
    let rem = inner.saturating_sub(title_block.len());
    let left = rem / 2;
    let right = rem.saturating_sub(left);
    format!("┌{}{}{}┐", "─".repeat(left), title_block, "─".repeat(right))
}

fn make_box_bottom(width: usize) -> String {
    if width < 2 {
        return String::new();
    }
    format!("└{}┘", "─".repeat(width.saturating_sub(2)))
}

fn role_prefix(role: &str) -> (&'static str, &'static str) {
    match role {
        "user" => ("you> ", "     "),
        "assistant" => ("pi > ", "     "),
        "tool" => ("tl > ", "     "),
        "warning" => ("wrn> ", "     "),
        "error" => ("err> ", "     "),
        _ => ("sys> ", "     "),
    }
}

fn style_chat_line(line: &str) -> String {
    if let Some(rest) = line.strip_prefix("you> ") {
        return format!("{}{}", style_accent("you> "), rest);
    }
    if let Some(rest) = line.strip_prefix("pi > ") {
        return format!("{}{}", style_accent("pi > "), rest);
    }
    if let Some(rest) = line.strip_prefix("tl > ") {
        return format!("{}{}", style_dim("tl > "), style_accent(rest));
    }
    if let Some(rest) = line.strip_prefix("wrn> ") {
        return format!("{}{}", style_warning("wrn> "), style_warning(rest));
    }
    if let Some(rest) = line.strip_prefix("err> ") {
        return format!("{}{}", style_error("err> "), style_error(rest));
    }
    if let Some(rest) = line.strip_prefix("sys> ") {
        return format!("{}{}", style_dim("sys> "), style_dim(rest));
    }
    line.to_string()
}

fn style_status(status: &str) -> String {
    if status.contains("error") || status.contains("auth required") {
        style_warning(status)
    } else if status.contains("thinking") || status.contains("approval") {
        style_accent(status)
    } else {
        style_accent(status)
    }
}

fn style_accent(s: &str) -> String {
    format!("\x1b[1;36m{s}\x1b[0m")
}

fn style_dim(s: &str) -> String {
    format!("\x1b[90m{s}\x1b[0m")
}

fn style_warning(s: &str) -> String {
    format!("\x1b[1;33m{s}\x1b[0m")
}

fn style_error(s: &str) -> String {
    format!("\x1b[1;31m{s}\x1b[0m")
}

fn wrap_plain_line(line: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![String::new()];
    }

    let chars: Vec<char> = line.chars().collect();
    if chars.is_empty() {
        return vec![String::new()];
    }

    let mut out = Vec::new();
    let mut start = 0usize;
    while start < chars.len() {
        let end = (start + max_width).min(chars.len());
        out.push(chars[start..end].iter().collect::<String>());
        start = end;
    }
    out
}

fn push_wrapped_message_lines(
    out: &mut Vec<String>,
    role: &str,
    lead: &str,
    cont: &str,
    text: &str,
    width: usize,
) {
    let lead_width = lead.chars().count();
    let cont_width = cont.chars().count();
    let lead_payload = width.saturating_sub(lead_width).max(1);
    let cont_payload = width.saturating_sub(cont_width).max(1);

    if role == "assistant" {
        let payload = lead_payload.min(cont_payload).max(1);
        let markdown = Markdown::new(text.to_string(), 0, 0, MarkdownTheme::default());
        let rendered = markdown.render(payload as u16);
        let mut wrote_first = false;
        for line in rendered {
            let prefix = if !wrote_first {
                wrote_first = true;
                lead
            } else {
                cont
            };
            out.push(format!("{prefix}{line}"));
        }
        if !wrote_first {
            out.push(format!("{lead}"));
        }
        return;
    }

    let mut wrote_first = false;
    for raw in text.lines() {
        let mut first_in_raw = true;
        let wrapped = if !wrote_first && first_in_raw {
            wrap_plain_line(raw, lead_payload)
        } else {
            wrap_plain_line(raw, cont_payload)
        };

        for chunk in wrapped {
            let prefix = if !wrote_first && first_in_raw {
                wrote_first = true;
                first_in_raw = false;
                lead
            } else {
                cont
            };
            out.push(format!("{prefix}{chunk}"));
        }
    }

    if !wrote_first {
        out.push(format!("{lead}"));
    }
}

fn pad_or_truncate_visible(s: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }

    let mut out = String::new();
    let mut visible = 0usize;
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            out.push(ch);
            if let Some(next) = chars.peek().copied() {
                out.push(next);
                chars.next();

                // CSI: ESC [ ... letter
                if next == '[' {
                    for c in chars.by_ref() {
                        out.push(c);
                        if c.is_ascii_alphabetic() {
                            break;
                        }
                    }
                    continue;
                }

                // APC/OSC-like controls used by cursor markers: ESC _ ... BEL
                if next == '_' {
                    for c in chars.by_ref() {
                        out.push(c);
                        if c == '\x07' {
                            break;
                        }
                    }
                    continue;
                }
            }
            continue;
        }

        if visible >= width {
            break;
        }
        out.push(ch);
        visible += 1;
    }

    if visible < width {
        out.push_str(&" ".repeat(width - visible));
    }
    out
}

/// Truncate a string to fit within a given width
fn truncate(s: &str, max_width: usize) -> String {
    if s.len() <= max_width {
        s.to_string()
    } else {
        let end = s
            .char_indices()
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
                if c.is_ascii_alphabetic() {
                    let lower = c.to_ascii_lowercase() as u8;
                    let code = lower.wrapping_sub(b'a').wrapping_add(1);
                    String::from(code as char)
                } else {
                    String::new()
                }
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
        KeyCode::BackTab => "\x1b[Z".to_string(),
        KeyCode::Tab => {
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                "\x1b[Z".to_string()
            } else {
                "\t".to_string()
            }
        }
        KeyCode::Esc => "\x1b".to_string(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn quit_on_ctrl_c() {
        // Verify that Ctrl-C sets should_quit.
        // We can't construct a full TuiApp without a real Agent, but we can
        // test the key-to-bytes conversion used for quit detection.
        let key = event::KeyEvent::new(event::KeyCode::Char('c'), event::KeyModifiers::CONTROL);
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

    #[test]
    fn thinking_cycle_wraps_back_to_off() {
        assert_eq!(cycle_thinking_level(None), Some(ThinkingLevel::Minimal));
        assert_eq!(
            cycle_thinking_level(Some(ThinkingLevel::Minimal)),
            Some(ThinkingLevel::Low)
        );
        assert_eq!(
            cycle_thinking_level(Some(ThinkingLevel::Low)),
            Some(ThinkingLevel::Medium)
        );
        assert_eq!(
            cycle_thinking_level(Some(ThinkingLevel::Medium)),
            Some(ThinkingLevel::High)
        );
        assert_eq!(
            cycle_thinking_level(Some(ThinkingLevel::High)),
            Some(ThinkingLevel::XHigh)
        );
        assert_eq!(cycle_thinking_level(Some(ThinkingLevel::XHigh)), None);
    }

    #[test]
    fn parse_thinking_level_accepts_expected_aliases() {
        assert_eq!(parse_thinking_level("off"), Some(None));
        assert_eq!(parse_thinking_level("none"), Some(None));
        assert_eq!(
            parse_thinking_level("minimal"),
            Some(Some(ThinkingLevel::Minimal))
        );
        assert_eq!(
            parse_thinking_level("xhigh"),
            Some(Some(ThinkingLevel::XHigh))
        );
        assert_eq!(parse_thinking_level("invalid"), None);
    }

    #[test]
    fn shift_tab_maps_to_esc_z_sequence() {
        let key = event::KeyEvent::new(event::KeyCode::BackTab, event::KeyModifiers::SHIFT);
        let bytes = crossterm_key_to_bytes(&key);
        assert_eq!(bytes, "\x1b[Z");
    }

    #[test]
    fn ctrl_shift_letter_maps_to_control_code() {
        let key = event::KeyEvent::new(event::KeyCode::Char('P'), event::KeyModifiers::CONTROL);
        let bytes = crossterm_key_to_bytes(&key);
        assert_eq!(bytes, "\x10");
    }

    #[test]
    fn slash_completion_suggests_core_commands() {
        let catalog = crate::skills::SkillCatalog::default();
        let completions = slash_completion_candidates("/pro", &catalog);
        assert!(completions.iter().any(|c| c == "/provider "));
        assert!(completions.iter().any(|c| c == "/providers"));
    }

    #[test]
    fn slash_completion_includes_skill_names() {
        let mut catalog = crate::skills::SkillCatalog::default();
        catalog.upsert(crate::skills::Skill {
            name: "agentation".to_string(),
            description: "demo skill".to_string(),
            path: PathBuf::from("skills/agentation/SKILL.md"),
            content: String::new(),
            metadata: crate::skills::SkillMetadata::default(),
        });

        let completions = slash_completion_candidates("/skill:age", &catalog);
        assert_eq!(completions, vec!["/skill:agentation".to_string()]);
    }

    #[test]
    fn tab_autocomplete_applies_common_prefix() {
        let catalog = crate::skills::SkillCatalog::default();
        let mut app = TuiApp::new();
        app.editor.set_value("/pro");

        let applied = apply_slash_autocomplete(&mut app, &catalog);
        assert!(applied);
        assert_eq!(app.editor.value(), "/provider");
    }

    #[test]
    fn exact_model_match_supports_provider_prefix() {
        let models = pi_ai::built_in_models().to_vec();
        let Some(anthropic) = models
            .iter()
            .find(|m| m.provider.to_string() == "anthropic")
            .cloned()
        else {
            panic!("anthropic model missing from built-in registry");
        };

        let matched =
            find_exact_model_match(&models, &format!("{}/{}", anthropic.provider, anthropic.id));
        assert_eq!(
            matched.as_ref().map(|m| m.id.as_str()),
            Some(anthropic.id.as_str())
        );
        assert_eq!(
            matched.as_ref().map(|m| m.provider.to_string()),
            Some(anthropic.provider.to_string())
        );
    }

    #[test]
    fn exact_model_match_requires_uniqueness_without_provider() {
        let Some(first) = pi_ai::built_in_models().first().cloned() else {
            panic!("no built-in models");
        };
        let mut second = first.clone();
        second.provider = pi_ai::messages::types::Provider::OpenAI;
        let models = vec![first, second];

        let matched = find_exact_model_match(&models, models[0].id.as_str());
        assert!(matched.is_none());
    }
}
