use crate::components::traits::{Component, InputResult};

/// A message chunk with associated metadata.
#[derive(Debug, Clone)]
pub struct MessageChunk {
    pub content: String,
    pub timestamp: std::time::Instant,
}

/// Streaming state indicator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamingState {
    /// Waiting to start
    Idle,
    /// Currently receiving chunks
    Streaming,
    /// Completed successfully
    Completed,
    /// Error occurred
    Error,
}

/// Theme for streaming message rendering.
pub struct StreamingTheme {
    /// Style for the message content
    pub content: Box<dyn Fn(&str) -> String + Send>,
    /// Style for the cursor/indicator during streaming
    pub cursor: Box<dyn Fn(&str) -> String + Send>,
    /// Style for completed state
    pub completed: Box<dyn Fn(&str) -> String + Send>,
    /// Style for error state
    pub error: Box<dyn Fn(&str) -> String + Send>,
    /// Style for timestamps
    pub timestamp: Box<dyn Fn(&str) -> String + Send>,
}

impl Default for StreamingTheme {
    fn default() -> Self {
        Self {
            content: Box::new(|s| s.to_string()),
            cursor: Box::new(|s| format!("\x1b[5m{}\x1b[0m", s)), // Blinking
            completed: Box::new(|s| format!("\x1b[32m{}\x1b[0m", s)),
            error: Box::new(|s| format!("\x1b[31m{}\x1b[0m", s)),
            timestamp: Box::new(|s| format!("\x1b[90m{}\x1b[0m", s)),
        }
    }
}

/// Streaming message component for displaying real-time AI responses.
///
/// Accumulates text chunks and renders them with an animated cursor
/// during streaming, and a completion indicator when done.
pub struct StreamingMessage {
    chunks: Vec<MessageChunk>,
    state: StreamingState,
    theme: StreamingTheme,
    dirty: bool,
    /// Maximum width for wrapping
    max_width: u16,
    /// Show streaming cursor
    show_cursor: bool,
    /// Cursor animation frame
    cursor_frame: usize,
    /// Show timestamp
    show_timestamp: bool,
    /// Start time for duration calculation
    start_time: Option<std::time::Instant>,
    /// End time for duration calculation
    end_time: Option<std::time::Instant>,
}

impl StreamingMessage {
    /// Create a new streaming message.
    pub fn new() -> Self {
        Self {
            chunks: Vec::new(),
            state: StreamingState::Idle,
            theme: StreamingTheme::default(),
            dirty: true,
            max_width: 80,
            show_cursor: true,
            cursor_frame: 0,
            show_timestamp: false,
            start_time: None,
            end_time: None,
        }
    }

    /// Set the theme.
    pub fn with_theme(mut self, theme: StreamingTheme) -> Self {
        self.theme = theme;
        self
    }

    /// Set maximum width for wrapping.
    pub fn with_max_width(mut self, width: u16) -> Self {
        self.max_width = width;
        self
    }

    /// Enable or disable cursor animation.
    pub fn with_cursor(mut self, show: bool) -> Self {
        self.show_cursor = show;
        self
    }

    /// Enable or disable timestamp display.
    pub fn with_timestamp(mut self, show: bool) -> Self {
        self.show_timestamp = show;
        self
    }

    /// Start streaming.
    pub fn start(&mut self) {
        self.state = StreamingState::Streaming;
        self.start_time = Some(std::time::Instant::now());
        self.dirty = true;
    }

    /// Append a chunk of content.
    pub fn append(&mut self, content: impl Into<String>) {
        let content = content.into();
        if !content.is_empty() {
            self.chunks.push(MessageChunk {
                content,
                timestamp: std::time::Instant::now(),
            });
            self.dirty = true;
        }
    }

    /// Complete the streaming.
    pub fn complete(&mut self) {
        self.state = StreamingState::Completed;
        self.end_time = Some(std::time::Instant::now());
        self.dirty = true;
    }

    /// Mark as error.
    pub fn error(&mut self, message: impl Into<String>) {
        self.state = StreamingState::Error;
        self.append(message);
        self.end_time = Some(std::time::Instant::now());
        self.dirty = true;
    }

    /// Get the full message content.
    pub fn content(&self) -> String {
        self.chunks.iter().map(|c| c.content.as_str()).collect()
    }

    /// Get current streaming state.
    pub fn state(&self) -> StreamingState {
        self.state
    }

    /// Check if streaming is active.
    pub fn is_streaming(&self) -> bool {
        self.state == StreamingState::Streaming
    }

    /// Get chunk count.
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// Get streaming duration.
    pub fn duration(&self) -> Option<std::time::Duration> {
        match (self.start_time, self.end_time) {
            (Some(start), Some(end)) => Some(end.duration_since(start)),
            (Some(start), None) => Some(std::time::Instant::now().duration_since(start)),
            _ => None,
        }
    }

    /// Reset to empty state.
    pub fn clear(&mut self) {
        self.chunks.clear();
        self.state = StreamingState::Idle;
        self.start_time = None;
        self.end_time = None;
        self.dirty = true;
    }

    /// Advance cursor animation frame.
    pub fn tick(&mut self) {
        if self.state == StreamingState::Streaming && self.show_cursor {
            self.cursor_frame = (self.cursor_frame + 1) % 4;
            self.dirty = true;
        }
    }

    fn get_cursor(&self) -> &'static str {
        match self.cursor_frame {
            0 => "▌",
            1 => "▐", 
            2 => "▌",
            _ => " ",
        }
    }

    fn format_duration(&self) -> String {
        if let Some(duration) = self.duration() {
            let secs = duration.as_secs();
            if secs < 60 {
                format!("{}.{:02}s", secs, duration.subsec_millis() / 10)
            } else {
                format!("{}:{:02}", secs / 60, secs % 60)
            }
        } else {
            String::new()
        }
    }

    fn wrap_content(&self, content: &str, width: usize) -> Vec<String> {
        let mut lines = Vec::new();
        
        for paragraph in content.split('\n') {
            if paragraph.is_empty() {
                lines.push(String::new());
                continue;
            }
            
            let mut current_line = String::new();
            let mut current_width = 0usize;
            
            for word in paragraph.split_whitespace() {
                let word_width = unicode_width::UnicodeWidthStr::width(word);
                
                if current_width + word_width + 1 > width {
                    if !current_line.is_empty() {
                        lines.push(current_line.clone());
                    }
                    current_line = word.to_string();
                    current_width = word_width;
                } else {
                    if !current_line.is_empty() {
                        current_line.push(' ');
                        current_width += 1;
                    }
                    current_line.push_str(word);
                    current_width += word_width;
                }
            }
            
            if !current_line.is_empty() {
                lines.push(current_line);
            }
        }
        
        if lines.is_empty() {
            lines.push(String::new());
        }
        
        lines
    }
}

impl Default for StreamingMessage {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for StreamingMessage {
    fn render(&self, width: u16) -> Vec<String> {
        let w = (width as usize).min(self.max_width as usize);
        let content = self.content();
        
        let mut lines = if content.is_empty() {
            vec![String::new()]
        } else {
            self.wrap_content(&content, w)
        };

        // Apply content styling
        for line in &mut lines {
            *line = (self.theme.content)(line);
        }

        // Add cursor if streaming
        if self.state == StreamingState::Streaming && self.show_cursor {
            if let Some(last) = lines.last_mut() {
                last.push_str(&format!(" {}", (self.theme.cursor)(self.get_cursor())));
            }
        }

        // Add status indicator
        let status = match self.state {
            StreamingState::Idle => None,
            StreamingState::Streaming => Some((self.theme.cursor)(" ○")),
            StreamingState::Completed => {
                let dur = self.format_duration();
                if self.show_timestamp && !dur.is_empty() {
                    Some((self.theme.completed)(&format!(" ✓ ({})", dur)))
                } else {
                    Some((self.theme.completed)(" ✓"))
                }
            }
            StreamingState::Error => Some((self.theme.error)(" ✗ Error")),
        };

        if let Some(status) = status {
            lines.push(status);
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

/// Collection of streaming messages (for chat-like interfaces).
pub struct StreamingMessageList {
    messages: Vec<StreamingMessage>,
    max_messages: usize,
    dirty: bool,
}

impl StreamingMessageList {
    /// Create a new message list.
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            max_messages: 100,
            dirty: true,
        }
    }

    /// Set maximum number of messages to keep.
    pub fn with_max_messages(mut self, max: usize) -> Self {
        self.max_messages = max;
        self
    }

    /// Add a new message.
    pub fn add_message(&mut self, message: StreamingMessage) {
        self.messages.push(message);
        if self.messages.len() > self.max_messages {
            self.messages.remove(0);
        }
        self.dirty = true;
    }

    /// Start a new streaming message.
    pub fn start_message(&mut self) -> &mut StreamingMessage {
        let mut msg = StreamingMessage::new();
        msg.start();
        self.add_message(msg);
        self.messages.last_mut().unwrap()
    }

    /// Get the current/active message.
    pub fn current_message(&mut self) -> Option<&mut StreamingMessage> {
        self.messages.last_mut()
    }

    /// Get all messages.
    pub fn messages(&self) -> &[StreamingMessage] {
        &self.messages
    }

    /// Clear all messages.
    pub fn clear(&mut self) {
        self.messages.clear();
        self.dirty = true;
    }

    /// Update animation for all streaming messages.
    pub fn tick(&mut self) {
        for msg in &mut self.messages {
            msg.tick();
        }
    }

    /// Check if any message is dirty.
    pub fn any_dirty(&self) -> bool {
        self.dirty || self.messages.iter().any(|m| m.is_dirty())
    }
}

impl Default for StreamingMessageList {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for StreamingMessageList {
    fn render(&self, width: u16) -> Vec<String> {
        let mut all_lines = Vec::new();
        
        for (i, msg) in self.messages.iter().enumerate() {
            let lines = msg.render(width);
            all_lines.extend(lines);
            
            // Add separator between messages (except after last)
            if i < self.messages.len() - 1 {
                all_lines.push(String::new());
            }
        }
        
        all_lines
    }

    fn handle_input(&mut self, _data: &str) -> InputResult {
        InputResult::Ignored
    }

    fn invalidate(&mut self) {
        self.dirty = true;
        for msg in &mut self.messages {
            msg.invalidate();
        }
    }

    fn is_dirty(&self) -> bool {
        self.any_dirty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_streaming_message() {
        let mut msg = StreamingMessage::new();
        assert_eq!(msg.state(), StreamingState::Idle);
        
        msg.start();
        assert_eq!(msg.state(), StreamingState::Streaming);
        
        msg.append("Hello");
        msg.append(" World");
        assert_eq!(msg.content(), "Hello World");
        
        msg.complete();
        assert_eq!(msg.state(), StreamingState::Completed);
        assert!(msg.duration().is_some());
    }

    #[test]
    fn test_streaming_render() {
        let mut msg = StreamingMessage::new();
        msg.start();
        msg.append("Test message");
        
        let lines = msg.render(80);
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_message_list() {
        let mut list = StreamingMessageList::new();
        
        let mut msg1 = StreamingMessage::new();
        msg1.start();
        msg1.append("First");
        msg1.complete();
        list.add_message(msg1);
        
        let mut msg2 = StreamingMessage::new();
        msg2.start();
        msg2.append("Second");
        list.add_message(msg2);
        
        assert_eq!(list.messages().len(), 2);
    }

    #[test]
    fn test_wrap_content() {
        let msg = StreamingMessage::new().with_max_width(20);
        let lines = msg.wrap_content("This is a long message that needs wrapping", 10);
        
        assert!(lines.len() > 1);
        for line in &lines {
            assert!(line.len() <= 10 || line.contains("message"));
        }
    }
}
