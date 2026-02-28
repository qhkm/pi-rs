use unicode_width::UnicodeWidthStr;
use crate::components::traits::{Component, InputResult};

/// Simple text display component with optional word wrapping.
pub struct Text {
    content: String,
    wrap: bool,
    dirty: bool,
}

impl Text {
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            wrap: true,
            dirty: true,
        }
    }

    /// Disable word wrapping (text is truncated at component width).
    pub fn no_wrap(mut self) -> Self {
        self.wrap = false;
        self
    }

    pub fn set_content(&mut self, content: impl Into<String>) {
        self.content = content.into();
        self.dirty = true;
    }

    pub fn content(&self) -> &str {
        &self.content
    }
}

impl Component for Text {
    fn render(&self, width: u16) -> Vec<String> {
        if width == 0 {
            return vec![];
        }

        let mut result = Vec::new();

        for raw_line in self.content.lines() {
            if self.wrap {
                let wrapped = word_wrap(raw_line, width as usize);
                result.extend(wrapped);
            } else {
                result.push(truncate_str(raw_line, width as usize));
            }
        }

        // Preserve trailing newline as blank line
        if self.content.ends_with('\n') {
            result.push(String::new());
        }

        result
    }

    fn invalidate(&mut self) {
        self.dirty = true;
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }
}

/// Text that truncates with an ellipsis when it exceeds the available width.
pub struct TruncatedText {
    content: String,
    dirty: bool,
}

impl TruncatedText {
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            dirty: true,
        }
    }

    pub fn set_content(&mut self, content: impl Into<String>) {
        self.content = content.into();
        self.dirty = true;
    }

    pub fn content(&self) -> &str {
        &self.content
    }
}

impl Component for TruncatedText {
    fn render(&self, width: u16) -> Vec<String> {
        if width == 0 {
            return vec![];
        }

        self.content
            .lines()
            .map(|line| truncate_with_ellipsis(line, width as usize))
            .collect()
    }

    fn invalidate(&mut self) {
        self.dirty = true;
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }
}

/// Wrap `text` to fit within `max_width` columns.
/// Splits on word boundaries and falls back to hard-wrapping long words.
pub fn word_wrap(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![String::new()];
    }

    let mut lines = Vec::new();
    let mut current_line = String::new();
    let mut current_width = 0usize;

    for word in text.split_inclusive(|c: char| c == ' ') {
        let word_width = UnicodeWidthStr::width(word);

        if current_width + word_width <= max_width {
            current_line.push_str(word);
            current_width += word_width;
        } else if word_width > max_width {
            // Hard-wrap long word
            if !current_line.is_empty() {
                lines.push(current_line.clone());
                current_line.clear();
                current_width = 0;
            }
            let mut remaining = word;
            while !remaining.is_empty() {
                let mut take = 0;
                let mut take_width = 0;
                for ch in remaining.chars() {
                    let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1);
                    if take_width + cw > max_width {
                        break;
                    }
                    take += ch.len_utf8();
                    take_width += cw;
                }
                if take == 0 {
                    break;
                }
                lines.push(remaining[..take].to_string());
                remaining = &remaining[take..];
            }
        } else {
            if !current_line.is_empty() {
                lines.push(current_line.trim_end().to_string());
                current_line.clear();
                current_width = 0;
            }
            current_line.push_str(word);
            current_width = word_width;
        }
    }

    if !current_line.is_empty() {
        lines.push(current_line);
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

/// Truncate `text` to fit within `max_width` columns.
pub fn truncate_str(text: &str, max_width: usize) -> String {
    let mut result = String::new();
    let mut width = 0;
    for ch in text.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1);
        if width + cw > max_width {
            break;
        }
        result.push(ch);
        width += cw;
    }
    result
}

/// Truncate `text` to fit within `max_width` columns, appending "…" if truncated.
pub fn truncate_with_ellipsis(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    let display_width = UnicodeWidthStr::width(text);
    if display_width <= max_width {
        return text.to_string();
    }

    // We need to fit text + ellipsis in max_width
    let ellipsis = "…";
    let ellipsis_width = UnicodeWidthStr::width(ellipsis);
    let available = max_width.saturating_sub(ellipsis_width);

    let mut result = String::new();
    let mut width = 0;
    for ch in text.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1);
        if width + cw > available {
            break;
        }
        result.push(ch);
        width += cw;
    }
    result.push_str(ellipsis);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_with_ellipsis() {
        assert_eq!(truncate_with_ellipsis("hello", 10), "hello");
        assert_eq!(truncate_with_ellipsis("hello world", 8), "hello w…");
    }

    #[test]
    fn test_word_wrap() {
        let lines = word_wrap("hello world foo", 7);
        assert!(!lines.is_empty());
    }
}
