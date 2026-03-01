use crate::components::traits::{Component, Focusable, InputResult, CURSOR_MARKER};
use crate::components::Input;
use crate::fuzzy::{fuzzy_filter, highlight_matches, FuzzyMatch, MatchOptions};
use crate::keyboard::keybindings::{EditorAction, KeybindingsManager};

/// Autocomplete component combining an input field with a dropdown of suggestions.
///
/// Supports fuzzy filtering, keyboard navigation, and custom highlighting.
pub struct Autocomplete {
    input: Input,
    suggestions: Vec<String>,
    filtered: Vec<FuzzyMatch>,
    selected_index: usize,
    max_visible: usize,
    visible: bool,
    dirty: bool,
    theme: AutocompleteTheme,
    keybindings: KeybindingsManager,
    /// Trigger characters (e.g., '@' for mentions, '/' for paths)
    triggers: Vec<char>,
    /// Current trigger context if any
    active_trigger: Option<char>,
    /// Callback when an item is selected
    pub on_select: Option<Box<dyn Fn(&str) + Send>>,
    /// Callback when the dropdown should be closed
    pub on_cancel: Option<Box<dyn Fn() + Send>>,
}

/// Theme for the autocomplete dropdown.
pub struct AutocompleteTheme {
    /// Style for the selected item
    pub selected: Box<dyn Fn(&str) -> String + Send>,
    /// Style for unselected items
    pub unselected: Box<dyn Fn(&str) -> String + Send>,
    /// Style for matched characters in suggestions
    pub highlight: Box<dyn Fn(&str) -> String + Send>,
    /// Style for the dropdown border
    pub border: Box<dyn Fn(&str) -> String + Send>,
    /// Style for "no matches" message
    pub no_matches: Box<dyn Fn(&str) -> String + Send>,
}

impl Default for AutocompleteTheme {
    fn default() -> Self {
        Self {
            selected: Box::new(|s| format!("\x1b[7m {} \x1b[0m", s)),
            unselected: Box::new(|s| format!(" {} ", s)),
            highlight: Box::new(|s| format!("\x1b[1;33m{}\x1b[0m", s)),
            border: Box::new(|s| format!("\x1b[90m{}\x1b[0m", s)),
            no_matches: Box::new(|s| format!("\x1b[90m {} \x1b[0m", s)),
        }
    }
}

impl Autocomplete {
    /// Create a new autocomplete with the given suggestions.
    pub fn new(suggestions: Vec<String>) -> Self {
        Self {
            input: Input::new(),
            suggestions,
            filtered: Vec::new(),
            selected_index: 0,
            max_visible: 10,
            visible: false,
            dirty: true,
            theme: AutocompleteTheme::default(),
            keybindings: KeybindingsManager::new(),
            triggers: vec!['@', '/', '~', ':'],
            active_trigger: None,
            on_select: None,
            on_cancel: None,
        }
    }

    /// Create an empty autocomplete.
    pub fn empty() -> Self {
        Self::new(Vec::new())
    }

    /// Set the theme.
    pub fn with_theme(mut self, theme: AutocompleteTheme) -> Self {
        self.theme = theme;
        self
    }

    /// Set the trigger characters.
    pub fn with_triggers(mut self, triggers: Vec<char>) -> Self {
        self.triggers = triggers;
        self
    }

    /// Set maximum visible items in dropdown.
    pub fn with_max_visible(mut self, max: usize) -> Self {
        self.max_visible = max;
        self
    }

    /// Set suggestions list.
    pub fn set_suggestions(&mut self, suggestions: Vec<String>) {
        self.suggestions = suggestions;
        self.filter_suggestions();
        self.dirty = true;
    }

    /// Add a single suggestion.
    pub fn add_suggestion(&mut self, suggestion: impl Into<String>) {
        self.suggestions.push(suggestion.into());
        self.dirty = true;
    }

    /// Get the current input value.
    pub fn value(&self) -> &str {
        self.input.value()
    }

    /// Set the input value.
    pub fn set_value(&mut self, value: impl Into<String>) {
        self.input.set_value(value);
        self.filter_suggestions();
        self.dirty = true;
    }

    /// Clear the input.
    pub fn clear(&mut self) {
        self.input.clear();
        self.filtered.clear();
        self.visible = false;
        self.dirty = true;
    }

    /// Show the dropdown.
    pub fn show(&mut self) {
        self.visible = true;
        self.filter_suggestions();
        self.dirty = true;
    }

    /// Hide the dropdown.
    pub fn hide(&mut self) {
        self.visible = false;
        self.dirty = true;
    }

    /// Check if dropdown is visible.
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Get the currently selected item.
    pub fn selected_item(&self) -> Option<&str> {
        self.filtered
            .get(self.selected_index)
            .map(|m| m.text.as_str())
    }

    /// Get all filtered matches.
    pub fn filtered_matches(&self) -> &[FuzzyMatch] {
        &self.filtered
    }

    fn filter_suggestions(&mut self) {
        let query = self.input.value();

        // Check for trigger context
        self.active_trigger = query.chars().next().and_then(|c| {
            if self.triggers.contains(&c) {
                Some(c)
            } else {
                None
            }
        });

        // Remove trigger character from search if present
        let search_query = if self.active_trigger.is_some() {
            &query[1..]
        } else {
            query
        };

        if search_query.is_empty() && self.active_trigger.is_none() {
            // Show all suggestions when empty (unless triggered)
            self.filtered = self
                .suggestions
                .iter()
                .map(|s| FuzzyMatch {
                    text: s.clone(),
                    score: 0,
                    positions: Vec::new(),
                })
                .collect();
        } else {
            // Filter with fuzzy matching
            let opts = MatchOptions {
                case_sensitive: false,
                word_boundary_only: false,
                max_gaps: None,
            };
            self.filtered = fuzzy_filter(search_query, &self.suggestions, &opts);
        }

        self.selected_index = 0;
        self.visible = !self.filtered.is_empty();
    }

    fn move_up(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        } else if !self.filtered.is_empty() {
            self.selected_index = self.filtered.len() - 1;
        }
        self.dirty = true;
    }

    fn move_down(&mut self) {
        if !self.filtered.is_empty() {
            self.selected_index = (self.selected_index + 1) % self.filtered.len();
        }
        self.dirty = true;
    }

    fn confirm_selection(&mut self) {
        if let Some(selected) = self.filtered.get(self.selected_index) {
            let value = if let Some(trigger) = self.active_trigger {
                format!("{}{}", trigger, selected.text)
            } else {
                selected.text.clone()
            };

            self.input.set_value(value);
            self.visible = false;

            if let Some(ref cb) = self.on_select {
                cb(&selected.text);
            }
        }
        self.dirty = true;
    }

    fn cancel(&mut self) {
        self.visible = false;
        self.dirty = true;
        if let Some(ref cb) = self.on_cancel {
            cb();
        }
    }

    /// Render just the input portion (for use when dropdown is hidden).
    pub fn render_input(&self, width: u16) -> Vec<String> {
        self.input.render(width)
    }

    /// Render the dropdown suggestions.
    pub fn render_dropdown(&self, width: u16) -> Vec<String> {
        if !self.visible || self.filtered.is_empty() {
            return Vec::new();
        }

        let mut lines = Vec::new();
        let w = width as usize;

        // Calculate visible range
        let visible_count = self.max_visible.min(self.filtered.len());
        let scroll_offset = if self.selected_index >= visible_count {
            self.selected_index - visible_count + 1
        } else {
            0
        };

        // Top border
        let border = (self.theme.border)(&"─".repeat(w.saturating_sub(2)));
        lines.push(format!("┌{}┐", border));

        for i in scroll_offset..(scroll_offset + visible_count).min(self.filtered.len()) {
            let item = &self.filtered[i];
            let is_selected = i == self.selected_index;

            // Highlight matched characters
            let highlighted = if item.positions.is_empty() {
                item.text.clone()
            } else {
                highlight_matches(&item.text, &item.positions, |s| (self.theme.highlight)(s))
            };

            // Apply selection style
            let styled = if is_selected {
                (self.theme.selected)(&highlighted)
            } else {
                (self.theme.unselected)(&highlighted)
            };

            // Pad or truncate to width
            let visible_len = strip_ansi_len(&styled);
            let line = if visible_len > w.saturating_sub(2) {
                truncate_visible(&styled, w.saturating_sub(2))
            } else {
                format!(
                    "{}{}",
                    styled,
                    " ".repeat(w.saturating_sub(2) - visible_len)
                )
            };

            lines.push(format!("│{}│", line));
        }

        // Bottom border
        lines.push(format!("└{}┘", border));

        lines
    }
}

impl Default for Autocomplete {
    fn default() -> Self {
        Self::empty()
    }
}

impl Component for Autocomplete {
    fn render(&self, width: u16) -> Vec<String> {
        let mut lines = self.input.render(width);

        if self.visible {
            lines.extend(self.render_dropdown(width));
        }

        lines
    }

    fn handle_input(&mut self, data: &str) -> InputResult {
        // Handle dropdown navigation
        if self.visible {
            let kb = &self.keybindings;

            if kb.matches(data, EditorAction::SelectUp) || kb.matches(data, EditorAction::CursorUp)
            {
                self.move_up();
                return InputResult::Consumed;
            }

            if kb.matches(data, EditorAction::SelectDown)
                || kb.matches(data, EditorAction::CursorDown)
            {
                self.move_down();
                return InputResult::Consumed;
            }

            if kb.matches(data, EditorAction::SelectConfirm) {
                self.confirm_selection();
                return InputResult::Consumed;
            }

            if kb.matches(data, EditorAction::SelectCancel) || data == "\x1b" {
                self.cancel();
                return InputResult::Consumed;
            }
        }

        // Pass through to input
        let result = self.input.handle_input(data);

        // Update filter after input changes
        self.filter_suggestions();

        result
    }

    fn invalidate(&mut self) {
        self.dirty = true;
        self.input.invalidate();
    }

    fn is_dirty(&self) -> bool {
        self.dirty || self.input.is_dirty()
    }
}

impl Focusable for Autocomplete {
    fn focused(&self) -> bool {
        self.input.focused()
    }

    fn set_focused(&mut self, focused: bool) {
        self.input.set_focused(focused);
        self.dirty = true;
    }
}

/// Get visual length of a string (excluding ANSI codes).
fn strip_ansi_len(s: &str) -> usize {
    use unicode_width::UnicodeWidthStr;

    let mut result = String::new();
    let mut chars = s.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                while let Some(c) = chars.next() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            result.push(ch);
        }
    }

    result.width()
}

/// Truncate a string to fit within a visual width.
fn truncate_visible(s: &str, max_width: usize) -> String {
    let mut result = String::new();
    let mut width = 0usize;
    let mut chars = s.chars().peekable();
    let mut in_ansi = false;

    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            result.push(ch);
            in_ansi = true;
            if chars.peek() == Some(&'[') {
                result.push(chars.next().unwrap());
            }
        } else if in_ansi {
            result.push(ch);
            if ch.is_ascii_alphabetic() {
                in_ansi = false;
            }
        } else {
            let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1);
            if width + cw > max_width {
                break;
            }
            result.push(ch);
            width += cw;
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_autocomplete_basic() {
        let suggestions = vec![
            "file1.txt".to_string(),
            "file2.txt".to_string(),
            "README.md".to_string(),
        ];

        let mut ac = Autocomplete::new(suggestions);
        ac.set_value("fi");
        ac.show();

        assert!(ac.is_visible());
        assert!(!ac.filtered.is_empty());
    }

    #[test]
    fn test_autocomplete_navigation() {
        let suggestions = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let mut ac = Autocomplete::new(suggestions);

        ac.show();
        assert_eq!(ac.selected_index, 0);

        ac.move_down();
        assert_eq!(ac.selected_index, 1);

        ac.move_up();
        assert_eq!(ac.selected_index, 0);
    }

    #[test]
    fn test_strip_ansi_len() {
        let plain = "Hello";
        assert_eq!(strip_ansi_len(plain), 5);

        let styled = "\x1b[31mHello\x1b[0m";
        assert_eq!(strip_ansi_len(styled), 5);
    }
}
