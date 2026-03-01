use crate::components::traits::{Component, InputResult};
use std::collections::HashMap;

/// State of a tool execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolState {
    /// Tool is pending execution
    Pending,
    /// Tool is currently running
    Running,
    /// Tool completed successfully
    Succeeded,
    /// Tool failed
    Failed,
    /// Tool was cancelled
    Cancelled,
}

/// A single tool execution.
#[derive(Debug, Clone)]
pub struct ToolExecution {
    /// Tool name
    pub name: String,
    /// Tool description
    pub description: String,
    /// Current state
    pub state: ToolState,
    /// Input parameters
    pub input: HashMap<String, String>,
    /// Output result
    pub output: Option<String>,
    /// Error message
    pub error: Option<String>,
    /// Start time
    pub start_time: Option<std::time::Instant>,
    /// End time
    pub end_time: Option<std::time::Instant>,
    /// Progress percentage (0-100)
    pub progress: Option<u8>,
    /// Nested sub-executions
    pub children: Vec<ToolExecution>,
}

impl ToolExecution {
    /// Create a new tool execution.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            state: ToolState::Pending,
            input: HashMap::new(),
            output: None,
            error: None,
            start_time: None,
            end_time: None,
            progress: None,
            children: Vec::new(),
        }
    }

    /// Set the input parameters.
    pub fn with_input(mut self, input: HashMap<String, String>) -> Self {
        self.input = input;
        self
    }

    /// Start the execution.
    pub fn start(&mut self) {
        self.state = ToolState::Running;
        self.start_time = Some(std::time::Instant::now());
    }

    /// Complete with success.
    pub fn complete(&mut self, output: impl Into<String>) {
        self.state = ToolState::Succeeded;
        self.output = Some(output.into());
        self.end_time = Some(std::time::Instant::now());
    }

    /// Complete with error.
    pub fn fail(&mut self, error: impl Into<String>) {
        self.state = ToolState::Failed;
        self.error = Some(error.into());
        self.end_time = Some(std::time::Instant::now());
    }

    /// Cancel the execution.
    pub fn cancel(&mut self) {
        self.state = ToolState::Cancelled;
        self.end_time = Some(std::time::Instant::now());
    }

    /// Set progress percentage.
    pub fn set_progress(&mut self, progress: u8) {
        self.progress = Some(progress.min(100));
    }

    /// Add a child execution.
    pub fn add_child(&mut self, child: ToolExecution) {
        self.children.push(child);
    }

    /// Get duration.
    pub fn duration(&self) -> Option<std::time::Duration> {
        match (self.start_time, self.end_time) {
            (Some(start), Some(end)) => Some(end.duration_since(start)),
            (Some(start), None) => Some(std::time::Instant::now().duration_since(start)),
            _ => None,
        }
    }

    /// Format duration as string.
    pub fn format_duration(&self) -> String {
        if let Some(dur) = self.duration() {
            let secs = dur.as_secs_f64();
            if secs < 1.0 {
                format!("{:.0}ms", secs * 1000.0)
            } else if secs < 60.0 {
                format!("{:.1}s", secs)
            } else {
                format!("{:.0}s", secs)
            }
        } else {
            String::new()
        }
    }
}

/// Theme for tool execution rendering.
pub struct ToolExecutionTheme {
    /// Style for pending state
    pub pending: Box<dyn Fn(&str) -> String + Send>,
    /// Style for running state
    pub running: Box<dyn Fn(&str) -> String + Send>,
    /// Style for success state
    pub success: Box<dyn Fn(&str) -> String + Send>,
    /// Style for failed state
    pub failed: Box<dyn Fn(&str) -> String + Send>,
    /// Style for cancelled state
    pub cancelled: Box<dyn Fn(&str) -> String + Send>,
    /// Style for tool name
    pub name: Box<dyn Fn(&str) -> String + Send>,
    /// Style for description
    pub description: Box<dyn Fn(&str) -> String + Send>,
    /// Style for parameters
    pub parameter: Box<dyn Fn(&str) -> String + Send>,
    /// Style for output
    pub output: Box<dyn Fn(&str) -> String + Send>,
    /// Style for error
    pub error: Box<dyn Fn(&str) -> String + Send>,
    /// Style for timing
    pub timing: Box<dyn Fn(&str) -> String + Send>,
    /// Style for borders
    pub border: Box<dyn Fn(&str) -> String + Send>,
}

impl Default for ToolExecutionTheme {
    fn default() -> Self {
        Self {
            pending: Box::new(|s| format!("\x1b[90m{}\x1b[0m", s)),
            running: Box::new(|s| format!("\x1b[33m{}\x1b[0m", s)),
            success: Box::new(|s| format!("\x1b[32m{}\x1b[0m", s)),
            failed: Box::new(|s| format!("\x1b[31m{}\x1b[0m", s)),
            cancelled: Box::new(|s| format!("\x1b[35m{}\x1b[0m", s)),
            name: Box::new(|s| format!("\x1b[1;97m{}\x1b[0m", s)),
            description: Box::new(|s| format!("\x1b[90m{}\x1b[0m", s)),
            parameter: Box::new(|s| format!("\x1b[36m{}\x1b[0m", s)),
            output: Box::new(|s| s.to_string()),
            error: Box::new(|s| format!("\x1b[31m{}\x1b[0m", s)),
            timing: Box::new(|s| format!("\x1b[90m{}\x1b[0m", s)),
            border: Box::new(|s| format!("\x1b[90m{}\x1b[0m", s)),
        }
    }
}

/// Component for visualizing tool executions.
///
/// Shows a hierarchical view of tools with their states, inputs, outputs,
/// and timing information.
pub struct ToolExecutionView {
    executions: Vec<ToolExecution>,
    theme: ToolExecutionTheme,
    dirty: bool,
    /// Whether to show expanded details
    expanded: HashMap<String, bool>,
    /// Show timing information
    show_timing: bool,
    /// Show input parameters
    show_input: bool,
    /// Show output
    show_output: bool,
    /// Maximum output lines to show
    max_output_lines: usize,
    /// Animation frame for running state
    anim_frame: usize,
}

impl ToolExecutionView {
    /// Create a new tool execution view.
    pub fn new() -> Self {
        Self {
            executions: Vec::new(),
            theme: ToolExecutionTheme::default(),
            dirty: true,
            expanded: HashMap::new(),
            show_timing: true,
            show_input: false,
            show_output: true,
            max_output_lines: 5,
            anim_frame: 0,
        }
    }

    /// Set the theme.
    pub fn with_theme(mut self, theme: ToolExecutionTheme) -> Self {
        self.theme = theme;
        self
    }

    /// Set timing display.
    pub fn with_timing(mut self, show: bool) -> Self {
        self.show_timing = show;
        self
    }

    /// Set input display.
    pub fn with_input(mut self, show: bool) -> Self {
        self.show_input = show;
        self
    }

    /// Set output display.
    pub fn with_output(mut self, show: bool) -> Self {
        self.show_output = show;
        self
    }

    /// Add a tool execution.
    pub fn add_execution(&mut self, execution: ToolExecution) {
        self.executions.push(execution);
        self.dirty = true;
    }

    /// Start a new execution.
    pub fn start_execution(
        &mut self,
        name: impl Into<String>,
        description: impl Into<String>,
    ) -> &mut ToolExecution {
        let mut exec = ToolExecution::new(name, description);
        exec.start();
        self.add_execution(exec);
        self.executions.last_mut().unwrap()
    }

    /// Get all executions.
    pub fn executions(&self) -> &[ToolExecution] {
        &self.executions
    }

    /// Clear all executions.
    pub fn clear(&mut self) {
        self.executions.clear();
        self.dirty = true;
    }

    /// Check if any tool is running.
    pub fn has_running(&self) -> bool {
        self.executions
            .iter()
            .any(|e| e.state == ToolState::Running)
    }

    /// Get count of completed tools.
    pub fn completed_count(&self) -> usize {
        self.executions
            .iter()
            .filter(|e| {
                matches!(
                    e.state,
                    ToolState::Succeeded | ToolState::Failed | ToolState::Cancelled
                )
            })
            .count()
    }

    /// Toggle expanded state for an execution.
    pub fn toggle_expanded(&mut self, id: &str) {
        let expanded = self.expanded.entry(id.to_string()).or_insert(false);
        *expanded = !*expanded;
        self.dirty = true;
    }

    /// Advance animation frame.
    pub fn tick(&mut self) {
        if self.has_running() {
            self.anim_frame = (self.anim_frame + 1) % 4;
            self.dirty = true;
        }
    }

    fn get_state_icon(&self, state: ToolState) -> &'static str {
        match state {
            ToolState::Pending => "○",
            ToolState::Running => self.get_anim_frame(),
            ToolState::Succeeded => "✓",
            ToolState::Failed => "✗",
            ToolState::Cancelled => "⊘",
        }
    }

    fn get_anim_frame(&self) -> &'static str {
        match self.anim_frame {
            0 => "◐",
            1 => "◓",
            2 => "◑",
            _ => "◒",
        }
    }

    fn render_execution(&self, exec: &ToolExecution, width: usize, depth: usize) -> Vec<String> {
        let mut lines = Vec::new();
        let indent = "  ".repeat(depth);
        let w = width.saturating_sub(indent.len());

        // Main tool line: [icon] Name - Description (timing)
        let icon = self.get_state_icon(exec.state);
        let styled_icon = match exec.state {
            ToolState::Pending => (self.theme.pending)(icon),
            ToolState::Running => (self.theme.running)(icon),
            ToolState::Succeeded => (self.theme.success)(icon),
            ToolState::Failed => (self.theme.failed)(icon),
            ToolState::Cancelled => (self.theme.cancelled)(icon),
        };

        let name = (self.theme.name)(&exec.name);
        let desc = (self.theme.description)(&exec.description);

        let timing = if self.show_timing {
            let dur = exec.format_duration();
            if dur.is_empty() {
                String::new()
            } else {
                format!(" {}", (self.theme.timing)(&format!("({})", dur)))
            }
        } else {
            String::new()
        };

        let progress = if let Some(p) = exec.progress {
            format!(" [{}%]", p)
        } else {
            String::new()
        };

        let main_line = format!(
            "{}{} {} - {}{}{}",
            indent, styled_icon, name, desc, timing, progress
        );
        lines.push(truncate_line(&main_line, width));

        // Input parameters (if expanded and enabled)
        if self.show_input && !exec.input.is_empty() {
            for (key, value) in &exec.input {
                let param = format!(
                    "{}  {}: {}",
                    indent,
                    (self.theme.parameter)(key),
                    truncate_string(value, w.saturating_sub(key.len() + 4))
                );
                lines.push(param);
            }
        }

        // Output (if enabled)
        if self.show_output {
            if let Some(output) = &exec.output {
                let output_lines: Vec<&str> = output.lines().collect();
                let show_lines = output_lines.len().min(self.max_output_lines);

                for i in 0..show_lines {
                    let line = format!("{}  {}", indent, (self.theme.output)(output_lines[i]));
                    lines.push(truncate_line(&line, width));
                }

                if output_lines.len() > self.max_output_lines {
                    let more = format!(
                        "{}  ... ({} more lines)",
                        indent,
                        output_lines.len() - self.max_output_lines
                    );
                    lines.push((self.theme.description)(&more));
                }
            }

            if let Some(error) = &exec.error {
                let err_line = format!("{}  Error: {}", indent, error);
                lines.push((self.theme.error)(&truncate_line(&err_line, width)));
            }
        }

        // Children (recursive)
        for child in &exec.children {
            lines.extend(self.render_execution(child, width, depth + 1));
        }

        lines
    }
}

impl Default for ToolExecutionView {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for ToolExecutionView {
    fn render(&self, width: u16) -> Vec<String> {
        let w = width as usize;
        let mut lines = Vec::new();

        if self.executions.is_empty() {
            return lines;
        }

        // Header
        let running = self
            .executions
            .iter()
            .filter(|e| e.state == ToolState::Running)
            .count();
        let completed = self.completed_count();
        let total = self.executions.len();

        let header = if running > 0 {
            format!(
                "Tools: {}/{} running, {} completed",
                running, total, completed
            )
        } else {
            format!("Tools: {}/{} completed", completed, total)
        };
        lines.push((self.theme.border)(&format!(
            "─{}",
            &"─".repeat(w.saturating_sub(1))
        )));
        lines.push(format!(" {}", (self.theme.name)(&header)));
        lines.push((self.theme.border)(&format!(
            "─{}",
            &"─".repeat(w.saturating_sub(1))
        )));

        // Executions
        for exec in &self.executions {
            lines.extend(self.render_execution(exec, w, 0));
        }

        lines
    }

    fn handle_input(&mut self, _data: &str) -> InputResult {
        InputResult::Ignored
    }

    fn invalidate(&mut self) {
        self.dirty = true;
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }
}

/// Simple tool execution spinner for inline use.
pub struct ToolSpinner {
    message: String,
    state: ToolState,
    anim_frame: usize,
    dirty: bool,
}

impl ToolSpinner {
    /// Create a new spinner.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            state: ToolState::Running,
            anim_frame: 0,
            dirty: true,
        }
    }

    /// Set state to success.
    pub fn success(&mut self) {
        self.state = ToolState::Succeeded;
        self.dirty = true;
    }

    /// Set state to error.
    pub fn error(&mut self) {
        self.state = ToolState::Failed;
        self.dirty = true;
    }

    /// Advance animation.
    pub fn tick(&mut self) {
        if self.state == ToolState::Running {
            self.anim_frame = (self.anim_frame + 1) % 8;
            self.dirty = true;
        }
    }

    fn get_spinner(&self) -> &'static str {
        const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];
        FRAMES[self.anim_frame]
    }
}

impl Default for ToolSpinner {
    fn default() -> Self {
        Self::new("Loading...")
    }
}

impl Component for ToolSpinner {
    fn render(&self, _width: u16) -> Vec<String> {
        let icon = match self.state {
            ToolState::Running => format!("\x1b[33m{}\x1b[0m", self.get_spinner()),
            ToolState::Succeeded => "\x1b[32m✓\x1b[0m".to_string(),
            ToolState::Failed => "\x1b[31m✗\x1b[0m".to_string(),
            _ => "○".to_string(),
        };

        vec![format!("{} {}", icon, self.message)]
    }

    fn handle_input(&mut self, _data: &str) -> InputResult {
        InputResult::Ignored
    }

    fn invalidate(&mut self) {
        self.dirty = true;
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }
}

fn truncate_line(line: &str, max_width: usize) -> String {
    if line.chars().count() <= max_width {
        line.to_string()
    } else {
        let truncated: String = line.chars().take(max_width.saturating_sub(1)).collect();
        format!("{}…", truncated)
    }
}

fn truncate_string(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_len.saturating_sub(1)).collect();
        format!("{}…", truncated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_execution() {
        let mut exec = ToolExecution::new("read", "Read file contents");
        assert_eq!(exec.state, ToolState::Pending);

        exec.start();
        assert_eq!(exec.state, ToolState::Running);

        exec.complete("File contents here");
        assert_eq!(exec.state, ToolState::Succeeded);
        assert_eq!(exec.output.as_ref().unwrap(), "File contents here");
        assert!(exec.duration().is_some());
    }

    #[test]
    fn test_tool_execution_fail() {
        let mut exec = ToolExecution::new("bash", "Run command");
        exec.start();
        exec.fail("Command not found");

        assert_eq!(exec.state, ToolState::Failed);
        assert_eq!(exec.error.unwrap(), "Command not found");
    }

    #[test]
    fn test_tool_view() {
        let mut view = ToolExecutionView::new();

        let exec = ToolExecution::new("grep", "Search files");
        view.add_execution(exec);

        assert_eq!(view.executions().len(), 1);
        assert!(!view.has_running());
    }

    #[test]
    fn test_tool_spinner() {
        let mut spinner = ToolSpinner::new("Working...");
        assert_eq!(spinner.state, ToolState::Running);

        spinner.success();
        assert_eq!(spinner.state, ToolState::Succeeded);
    }
}
