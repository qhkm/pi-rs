use crate::components::traits::{Component, InputResult};
use unicode_width::UnicodeWidthStr;

/// Footer component displaying status information like tokens, cost, and model.
///
/// Renders a status bar with multiple items, each with a label and value.
/// Automatically handles truncation and padding to fill the available width.
pub struct Footer {
    items: Vec<(String, String)>,
    dirty: bool,
    separator: String,
    theme: FooterTheme,
}

/// Theme for the footer component.
pub struct FooterTheme {
    /// Style for labels (e.g., "Tokens", "Cost")
    pub label_style: Box<dyn Fn(&str) -> String + Send>,
    /// Style for values (e.g., "1500", "$0.02")
    pub value_style: Box<dyn Fn(&str) -> String + Send>,
    /// Style for the separator between items
    pub separator_style: Box<dyn Fn(&str) -> String + Send>,
    /// Style for filler space
    pub filler_style: Box<dyn Fn(&str) -> String + Send>,
}

impl Default for FooterTheme {
    fn default() -> Self {
        Self {
            label_style: Box::new(|s| format!("\x1b[90m{}\x1b[0m", s)),
            value_style: Box::new(|s| format!("\x1b[97m{}\x1b[0m", s)),
            separator_style: Box::new(|s| format!("\x1b[90m{}\x1b[0m", s)),
            filler_style: Box::new(|s| format!("\x1b[40m{}\x1b[0m", s)),
        }
    }
}

impl Footer {
    /// Create a new footer with default theme.
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            dirty: true,
            separator: " │ ".to_string(),
            theme: FooterTheme::default(),
        }
    }

    /// Create a footer with custom theme.
    pub fn with_theme(mut self, theme: FooterTheme) -> Self {
        self.theme = theme;
        self
    }

    /// Set the separator string between items.
    pub fn with_separator(mut self, sep: impl Into<String>) -> Self {
        self.separator = sep.into();
        self
    }

    /// Set all items at once.
    pub fn set_items(&mut self, items: Vec<(impl Into<String>, impl Into<String>)>) {
        self.items = items
            .into_iter()
            .map(|(l, v)| (l.into(), v.into()))
            .collect();
        self.dirty = true;
    }

    /// Add or update an item by label.
    pub fn set_item(&mut self, label: impl Into<String>, value: impl Into<String>) {
        let label = label.into();
        let value = value.into();
        
        if let Some(pos) = self.items.iter().position(|(l, _)| l == &label) {
            self.items[pos].1 = value;
        } else {
            self.items.push((label, value));
        }
        self.dirty = true;
    }

    /// Remove an item by label.
    pub fn remove_item(&mut self, label: &str) {
        self.items.retain(|(l, _)| l != label);
        self.dirty = true;
    }

    /// Clear all items.
    pub fn clear(&mut self) {
        self.items.clear();
        self.dirty = true;
    }

    /// Set token count display.
    pub fn set_tokens(&mut self, input: u64, output: u64, total: u64) {
        self.set_item("Tokens", format!("{}/{} ({})", input, output, total));
    }

    /// Set cost display.
    pub fn set_cost(&mut self, cost: f64, currency: &str) {
        self.set_item("Cost", format!("{}{:.4}", currency, cost));
    }

    /// Set model display.
    pub fn set_model(&mut self, model: impl Into<String>) {
        self.set_item("Model", model.into());
    }

    /// Set thinking level display.
    pub fn set_thinking(&mut self, level: impl Into<String>) {
        self.set_item("Thinking", level.into());
    }

    /// Set status message.
    pub fn set_status(&mut self, status: impl Into<String>) {
        self.set_item("Status", status.into());
    }
}

impl Default for Footer {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for Footer {
    fn render(&self, width: u16) -> Vec<String> {
        let w = width as usize;
        let mut result = String::new();
        let mut visual_width = 0usize;

        for (i, (label, value)) in self.items.iter().enumerate() {
            let sep = if i > 0 { &self.separator } else { "" };
            let label_styled = (self.theme.label_style)(label);
            let value_styled = (self.theme.value_style)(value);
            let sep_styled = (self.theme.separator_style)(sep);
            
            let item_text = format!("{} {}: {}", sep_styled, label_styled, value_styled);
            let item_visual = format!("{}{}: {}", sep, label, value);
            let item_width = item_visual.width();

            if visual_width + item_width > w {
                // Truncate if needed
                let remaining = w.saturating_sub(visual_width);
                if remaining > 3 {
                    let truncated = truncate_visible(&item_text, remaining);
                    result.push_str(&truncated);
                    visual_width += remaining;
                }
                break;
            }

            result.push_str(&item_text);
            visual_width += item_width;
        }

        // Pad to fill width
        if visual_width < w {
            let padding = " ".repeat(w - visual_width);
            result.push_str(&(self.theme.filler_style)(&padding));
        }

        vec![result]
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

/// Truncate a string to fit within a visual width, preserving ANSI codes.
fn truncate_visible(s: &str, max_width: usize) -> String {
    let mut result = String::new();
    let mut width = 0usize;
    let mut chars = s.chars().peekable();

    while let Some(ch) = chars.next() {
        // Handle ANSI escape sequences
        if ch == '\x1b' {
            result.push(ch);
            if chars.peek() == Some(&'[') {
                result.push(chars.next().unwrap());
                // Consume until letter
                while let Some(c) = chars.next() {
                    result.push(c);
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }

        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1);
        if width + cw > max_width {
            break;
        }
        result.push(ch);
        width += cw;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_footer_basic() {
        let mut footer = Footer::new();
        footer.set_item("Tokens", "1000");
        footer.set_item("Model", "gpt-4");
        
        let lines = footer.render(50);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("Tokens"));
        assert!(lines[0].contains("1000"));
        assert!(lines[0].contains("Model"));
        assert!(lines[0].contains("gpt-4"));
    }

    #[test]
    fn test_footer_helpers() {
        let mut footer = Footer::new();
        footer.set_tokens(500, 200, 700);
        footer.set_cost(0.0234, "$");
        footer.set_model("claude-3-opus");
        footer.set_thinking("High");
        
        let lines = footer.render(80);
        assert!(lines[0].contains("500"));
        assert!(lines[0].contains("claude-3-opus"));
        assert!(lines[0].contains("High"));
    }

    #[test]
    fn test_truncate_visible() {
        let s = "Hello World";
        assert_eq!(truncate_visible(s, 5), "Hello");
        assert_eq!(truncate_visible(s, 8), "Hello Wo");
        
        // With ANSI codes - preserves opening codes
        let ansi = "\x1b[31mHello\x1b[0m World";
        let truncated = truncate_visible(ansi, 3);
        assert!(truncated.contains("\x1b[31m"));
        assert!(truncated.contains("Hel"));
    }
}
