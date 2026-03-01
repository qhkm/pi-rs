use crate::components::traits::{Component, Focusable, InputResult, CURSOR_MARKER};
use crate::keyboard::keybindings::{EditorAction, KeybindingsManager};
use crate::keyboard::kitty::{parse_input, Key, KeyEventType, Modifiers};
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};

#[derive(Debug, Clone)]
pub struct Selection {
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
}

#[derive(Debug, Clone)]
struct EditorSnapshot {
    lines: Vec<String>,
    cursor_line: usize,
    cursor_col: usize,
}

/// Multi-line text editor component.
///
/// Features:
/// - Multi-line text editing with cursor tracking
/// - Vertical scrolling with viewport
/// - Horizontal scrolling for long lines
/// - Syntax highlighting (placeholder — requires syntect integration)
/// - Selection support
/// - Undo/redo stack
/// - Kill ring
/// - Submit and Escape callbacks
pub struct Editor {
    lines: Vec<String>,
    cursor_line: usize,
    cursor_col: usize,
    scroll_top: usize,
    scroll_left: usize,
    height: u16,
    language: String,
    focused: bool,
    dirty: bool,
    selection: Option<Selection>,
    undo_stack: Vec<EditorSnapshot>,
    redo_stack: Vec<EditorSnapshot>,
    kill_ring: Vec<String>,
    kill_ring_index: usize,
    keybindings: KeybindingsManager,
    /// Key sequence that triggers submit (default: Enter in single-line-ish mode)
    submit_key: EditorAction,
    /// Key sequence that inserts a newline (default: Shift+Enter)
    newline_key: EditorAction,
    pub on_submit: Option<Box<dyn Fn(&str) + Send>>,
    pub on_escape: Option<Box<dyn Fn() + Send>>,
    // Syntax highlighting
    syntax_set: SyntaxSet,
    theme: Theme,
    syntax: Option<SyntaxReference>,
}

impl Editor {
    pub fn new(height: u16) -> Self {
        let syntax_set = SyntaxSet::load_defaults_newlines();
        let theme = ThemeSet::load_defaults()
            .themes
            .remove("base16-ocean.dark")
            .unwrap_or_else(|| {
                ThemeSet::load_defaults()
                    .themes
                    .into_values()
                    .next()
                    .expect("syntect must have at least one default theme")
            });

        Self {
            lines: vec![String::new()],
            cursor_line: 0,
            cursor_col: 0,
            scroll_top: 0,
            scroll_left: 0,
            height,
            language: String::new(),
            focused: false,
            dirty: true,
            selection: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            kill_ring: Vec::new(),
            kill_ring_index: 0,
            keybindings: KeybindingsManager::new(),
            submit_key: EditorAction::Submit,
            newline_key: EditorAction::NewLine,
            on_submit: None,
            on_escape: None,
            syntax_set,
            theme,
            syntax: None,
        }
    }

    pub fn with_language(mut self, language: impl Into<String>) -> Self {
        let lang = language.into();
        self.syntax = self
            .syntax_set
            .find_syntax_by_token(&lang)
            .cloned();
        self.language = lang;
        self
    }

    pub fn value(&self) -> String {
        self.lines.join("\n")
    }

    pub fn set_value(&mut self, value: impl Into<String>) {
        let s = value.into();
        self.lines = s.lines().map(|l| l.to_string()).collect();
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_line = 0;
        self.cursor_col = 0;
        self.scroll_top = 0;
        self.scroll_left = 0;
        self.dirty = true;
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    pub fn set_height(&mut self, height: u16) {
        self.height = height;
        self.dirty = true;
    }

    pub fn selection(&self) -> Option<&Selection> {
        self.selection.as_ref()
    }

    // -----------------------------------------------------------------------
    // Undo / redo
    // -----------------------------------------------------------------------

    fn push_undo(&mut self) {
        let snapshot = EditorSnapshot {
            lines: self.lines.clone(),
            cursor_line: self.cursor_line,
            cursor_col: self.cursor_col,
        };
        self.undo_stack.push(snapshot);
        self.redo_stack.clear();
        if self.undo_stack.len() > 200 {
            self.undo_stack.remove(0);
        }
    }

    fn undo(&mut self) {
        if let Some(snap) = self.undo_stack.pop() {
            let current = EditorSnapshot {
                lines: self.lines.clone(),
                cursor_line: self.cursor_line,
                cursor_col: self.cursor_col,
            };
            self.redo_stack.push(current);
            self.lines = snap.lines;
            self.cursor_line = snap.cursor_line;
            self.cursor_col = snap.cursor_col;
            self.dirty = true;
        }
    }

    fn redo(&mut self) {
        if let Some(snap) = self.redo_stack.pop() {
            let current = EditorSnapshot {
                lines: self.lines.clone(),
                cursor_line: self.cursor_line,
                cursor_col: self.cursor_col,
            };
            self.undo_stack.push(current);
            self.lines = snap.lines;
            self.cursor_line = snap.cursor_line;
            self.cursor_col = snap.cursor_col;
            self.dirty = true;
        }
    }

    // -----------------------------------------------------------------------
    // Cursor helpers
    // -----------------------------------------------------------------------

    fn current_line(&self) -> &str {
        self.lines
            .get(self.cursor_line)
            .map(|s| s.as_str())
            .unwrap_or("")
    }

    fn clamp_cursor_col(&mut self) {
        let line_len = self
            .lines
            .get(self.cursor_line)
            .map(|l| l.len())
            .unwrap_or(0);
        if self.cursor_col > line_len {
            self.cursor_col = line_len;
        }
    }

    fn ensure_scroll(&mut self, width: u16) {
        // Vertical scroll
        if self.cursor_line < self.scroll_top {
            self.scroll_top = self.cursor_line;
        }
        let visible_rows = self.height as usize;
        if self.cursor_line >= self.scroll_top + visible_rows {
            self.scroll_top = self.cursor_line - visible_rows + 1;
        }

        // Horizontal scroll (simple byte-based for now)
        if self.cursor_col < self.scroll_left {
            self.scroll_left = self.cursor_col;
        }
        let visible_cols = width as usize;
        if self.cursor_col >= self.scroll_left + visible_cols {
            self.scroll_left = self.cursor_col - visible_cols + 1;
        }
    }

    fn char_boundary_left(s: &str, pos: usize) -> usize {
        if pos == 0 {
            return 0;
        }
        let mut p = pos - 1;
        while !s.is_char_boundary(p) {
            if p == 0 {
                return 0;
            }
            p -= 1;
        }
        p
    }

    fn char_boundary_right(s: &str, pos: usize) -> usize {
        if pos >= s.len() {
            return s.len();
        }
        let mut p = pos + 1;
        while p <= s.len() && !s.is_char_boundary(p) {
            p += 1;
        }
        p
    }

    fn word_left(s: &str, pos: usize) -> usize {
        let substr = &s[..pos];
        let chars: Vec<char> = substr.chars().collect();
        let mut i = chars.len();
        while i > 0 && chars[i - 1] == ' ' {
            i -= 1;
        }
        while i > 0 && chars[i - 1] != ' ' {
            i -= 1;
        }
        substr
            .char_indices()
            .nth(i)
            .map(|(idx, _)| idx)
            .unwrap_or(0)
    }

    fn word_right(s: &str, pos: usize) -> usize {
        let substr = &s[pos..];
        let mut past_word = false;
        for (idx, ch) in substr.char_indices() {
            if ch != ' ' {
                past_word = true;
            }
            if past_word && ch == ' ' {
                return pos + idx;
            }
        }
        s.len()
    }

    // -----------------------------------------------------------------------
    // Editing operations
    // -----------------------------------------------------------------------

    fn insert_char(&mut self, ch: char) {
        self.push_undo();
        let line = &mut self.lines[self.cursor_line];
        line.insert(self.cursor_col, ch);
        self.cursor_col += ch.len_utf8();
        self.dirty = true;
    }

    fn insert_newline(&mut self) {
        self.push_undo();
        let rest = self.lines[self.cursor_line][self.cursor_col..].to_string();
        self.lines[self.cursor_line].truncate(self.cursor_col);
        self.cursor_line += 1;
        self.lines.insert(self.cursor_line, rest);
        self.cursor_col = 0;
        self.dirty = true;
    }

    fn delete_char_backward(&mut self) {
        if self.cursor_col > 0 {
            self.push_undo();
            let line = &mut self.lines[self.cursor_line];
            let prev = Self::char_boundary_left(line, self.cursor_col);
            line.drain(prev..self.cursor_col);
            self.cursor_col = prev;
            self.dirty = true;
        } else if self.cursor_line > 0 {
            self.push_undo();
            let line = self.lines.remove(self.cursor_line);
            self.cursor_line -= 1;
            self.cursor_col = self.lines[self.cursor_line].len();
            self.lines[self.cursor_line].push_str(&line);
            self.dirty = true;
        }
    }

    fn delete_char_forward(&mut self) {
        let line_len = self.lines[self.cursor_line].len();
        if self.cursor_col < line_len {
            self.push_undo();
            let next = Self::char_boundary_right(&self.lines[self.cursor_line], self.cursor_col);
            self.lines[self.cursor_line].drain(self.cursor_col..next);
            self.dirty = true;
        } else if self.cursor_line + 1 < self.lines.len() {
            self.push_undo();
            let next_line = self.lines.remove(self.cursor_line + 1);
            self.lines[self.cursor_line].push_str(&next_line);
            self.dirty = true;
        }
    }

    fn delete_word_backward(&mut self) {
        let line = self.lines[self.cursor_line].clone();
        let new_col = Self::word_left(&line, self.cursor_col);
        if new_col < self.cursor_col {
            self.push_undo();
            let deleted = line[new_col..self.cursor_col].to_string();
            self.lines[self.cursor_line].drain(new_col..self.cursor_col);
            self.cursor_col = new_col;
            self.push_kill(deleted);
            self.dirty = true;
        }
    }

    fn delete_word_forward(&mut self) {
        let line = self.lines[self.cursor_line].clone();
        let new_col = Self::word_right(&line, self.cursor_col);
        if new_col > self.cursor_col {
            self.push_undo();
            let deleted = line[self.cursor_col..new_col].to_string();
            self.lines[self.cursor_line].drain(self.cursor_col..new_col);
            self.push_kill(deleted);
            self.dirty = true;
        }
    }

    fn kill_to_end(&mut self) {
        let line_len = self.lines[self.cursor_line].len();
        if self.cursor_col < line_len {
            self.push_undo();
            let killed = self.lines[self.cursor_line][self.cursor_col..].to_string();
            self.lines[self.cursor_line].truncate(self.cursor_col);
            self.push_kill(killed);
            self.dirty = true;
        } else if self.cursor_line + 1 < self.lines.len() {
            // Kill the newline joining lines
            self.push_undo();
            let next = self.lines.remove(self.cursor_line + 1);
            self.lines[self.cursor_line].push_str(&next);
            self.push_kill("\n".to_string());
            self.dirty = true;
        }
    }

    fn kill_to_start(&mut self) {
        if self.cursor_col > 0 {
            self.push_undo();
            let killed = self.lines[self.cursor_line][..self.cursor_col].to_string();
            self.lines[self.cursor_line].drain(..self.cursor_col);
            self.cursor_col = 0;
            self.push_kill(killed);
            self.dirty = true;
        }
    }

    fn push_kill(&mut self, text: String) {
        self.kill_ring.push(text);
        self.kill_ring_index = self.kill_ring.len() - 1;
    }

    fn yank(&mut self) {
        if self.kill_ring.is_empty() {
            return;
        }
        let text = self.kill_ring[self.kill_ring_index].clone();
        self.push_undo();
        let line = &mut self.lines[self.cursor_line];
        line.insert_str(self.cursor_col, &text);
        self.cursor_col += text.len();
        self.dirty = true;
    }
}

impl Default for Editor {
    fn default() -> Self {
        Self::new(24)
    }
}

impl Component for Editor {
    fn render(&self, width: u16) -> Vec<String> {
        let mut result = Vec::new();
        let visible_rows = self.height as usize;
        let visible_cols = width as usize;

        // If a syntax is set, use syntect to highlight lines
        // Pre-compute highlighted spans for visible lines when syntax is set.
        // Each inner Vec contains (Style, String) pairs for one line.
        let highlighted: Option<Vec<Vec<(syntect::highlighting::Style, String)>>> =
            self.syntax.as_ref().map(|syntax| {
                use syntect::easy::HighlightLines;
                let mut h = HighlightLines::new(syntax, &self.theme);
                let end = (self.scroll_top + visible_rows).min(self.lines.len());
                let mut visible_styles = Vec::new();
                for (i, line) in self.lines[..end].iter().enumerate() {
                    let line_with_nl = format!("{line}\n");
                    let spans: Vec<(syntect::highlighting::Style, String)> = h
                        .highlight_line(&line_with_nl, &self.syntax_set)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|(style, text)| (style, text.to_string()))
                        .collect();
                    if i >= self.scroll_top {
                        visible_styles.push(spans);
                    }
                }
                visible_styles
            });

        for row_idx in 0..visible_rows {
            let line_idx = self.scroll_top + row_idx;
            let line = match self.lines.get(line_idx) {
                Some(l) => l,
                None => {
                    result.push(String::new());
                    continue;
                }
            };

            // Horizontal slice
            let visible_part = if self.scroll_left < line.len() {
                &line[self.scroll_left..]
            } else {
                ""
            };

            let mut rendered = String::new();

            if let Some(ref hl) = highlighted {
                if let Some(spans) = hl.get(row_idx) {
                    // Render with ANSI 24-bit color codes
                    let mut col_width = 0usize;
                    let mut byte_offset = 0usize;
                    let mut span_byte_pos = 0usize; // position within the full line+\n

                    for (style, text) in spans.iter() {
                        for ch in text.chars() {
                            if ch == '\n' {
                                span_byte_pos += 1;
                                continue;
                            }

                            // Skip chars before scroll_left
                            if span_byte_pos < self.scroll_left {
                                span_byte_pos += ch.len_utf8();
                                continue;
                            }

                            let cw =
                                unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1);

                            // Insert cursor marker
                            if self.focused
                                && line_idx == self.cursor_line
                                && self.scroll_left + byte_offset == self.cursor_col
                            {
                                rendered.push_str(CURSOR_MARKER);
                            }

                            if col_width + cw > visible_cols {
                                break;
                            }

                            let fg = style.foreground;
                            rendered.push_str(&format!(
                                "\x1b[38;2;{};{};{}m{}",
                                fg.r, fg.g, fg.b, ch
                            ));
                            col_width += cw;
                            byte_offset += ch.len_utf8();
                            span_byte_pos += ch.len_utf8();
                        }
                    }

                    // Reset color after the line
                    rendered.push_str("\x1b[0m");

                    // Cursor at end of line
                    if self.focused
                        && line_idx == self.cursor_line
                        && self.cursor_col >= self.scroll_left + byte_offset
                    {
                        rendered.push_str(CURSOR_MARKER);
                    }

                    result.push(rendered);
                    continue;
                }
            }

            // Fallback: plain text rendering (no syntax highlighting)
            let mut col_width = 0usize;
            let mut byte_offset = 0usize;

            for ch in visible_part.chars() {
                let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1);

                // Insert cursor marker at cursor position
                if self.focused
                    && line_idx == self.cursor_line
                    && self.scroll_left + byte_offset == self.cursor_col
                {
                    rendered.push_str(CURSOR_MARKER);
                }

                if col_width + cw > visible_cols {
                    break;
                }

                rendered.push(ch);
                col_width += cw;
                byte_offset += ch.len_utf8();
            }

            // Cursor at end of line
            if self.focused
                && line_idx == self.cursor_line
                && self.cursor_col >= self.scroll_left + byte_offset
            {
                rendered.push_str(CURSOR_MARKER);
            }

            result.push(rendered);
        }

        result
    }

    fn handle_input(&mut self, data: &str) -> InputResult {
        if !self.focused {
            return InputResult::Ignored;
        }

        // Bracketed paste
        if data.starts_with("\x1b[200~") {
            let rest = &data[6..];
            let end = rest.find("\x1b[201~").unwrap_or(rest.len());
            let paste = &rest[..end];
            self.push_undo();
            for ch in paste.chars() {
                if ch == '\n' || ch == '\r' {
                    self.insert_newline();
                } else {
                    self.insert_char(ch);
                }
            }
            self.dirty = true;
            return InputResult::Consumed;
        }

        // Escape
        if data == "\x1b" {
            if let Some(ref cb) = self.on_escape {
                cb();
            }
            return InputResult::Consumed;
        }

        let kb = &self.keybindings;

        if kb.matches(data, EditorAction::Submit) {
            if let Some(ref cb) = self.on_submit {
                let v = self.value();
                cb(&v);
            }
            return InputResult::Consumed;
        }

        if kb.matches(data, EditorAction::NewLine) {
            self.insert_newline();
            return InputResult::Consumed;
        }

        if kb.matches(data, EditorAction::CursorUp) {
            if self.cursor_line > 0 {
                self.cursor_line -= 1;
                self.clamp_cursor_col();
            }
            self.dirty = true;
            return InputResult::Consumed;
        }

        if kb.matches(data, EditorAction::CursorDown) {
            if self.cursor_line + 1 < self.lines.len() {
                self.cursor_line += 1;
                self.clamp_cursor_col();
            }
            self.dirty = true;
            return InputResult::Consumed;
        }

        if kb.matches(data, EditorAction::CursorLeft) {
            if self.cursor_col > 0 {
                let line = self.lines[self.cursor_line].clone();
                self.cursor_col = Self::char_boundary_left(&line, self.cursor_col);
            } else if self.cursor_line > 0 {
                self.cursor_line -= 1;
                self.cursor_col = self.lines[self.cursor_line].len();
            }
            self.dirty = true;
            return InputResult::Consumed;
        }

        if kb.matches(data, EditorAction::CursorRight) {
            let line_len = self.lines[self.cursor_line].len();
            if self.cursor_col < line_len {
                let line = self.lines[self.cursor_line].clone();
                self.cursor_col = Self::char_boundary_right(&line, self.cursor_col);
            } else if self.cursor_line + 1 < self.lines.len() {
                self.cursor_line += 1;
                self.cursor_col = 0;
            }
            self.dirty = true;
            return InputResult::Consumed;
        }

        if kb.matches(data, EditorAction::CursorWordLeft) {
            let line = self.lines[self.cursor_line].clone();
            self.cursor_col = Self::word_left(&line, self.cursor_col);
            self.dirty = true;
            return InputResult::Consumed;
        }

        if kb.matches(data, EditorAction::CursorWordRight) {
            let line = self.lines[self.cursor_line].clone();
            self.cursor_col = Self::word_right(&line, self.cursor_col);
            self.dirty = true;
            return InputResult::Consumed;
        }

        if kb.matches(data, EditorAction::CursorLineStart) {
            self.cursor_col = 0;
            self.dirty = true;
            return InputResult::Consumed;
        }

        if kb.matches(data, EditorAction::CursorLineEnd) {
            self.cursor_col = self.lines[self.cursor_line].len();
            self.dirty = true;
            return InputResult::Consumed;
        }

        if kb.matches(data, EditorAction::PageUp) {
            let step = self.height as usize;
            self.cursor_line = self.cursor_line.saturating_sub(step);
            self.clamp_cursor_col();
            self.dirty = true;
            return InputResult::Consumed;
        }

        if kb.matches(data, EditorAction::PageDown) {
            let step = self.height as usize;
            self.cursor_line = (self.cursor_line + step).min(self.lines.len().saturating_sub(1));
            self.clamp_cursor_col();
            self.dirty = true;
            return InputResult::Consumed;
        }

        if kb.matches(data, EditorAction::DeleteCharBackward) {
            self.delete_char_backward();
            return InputResult::Consumed;
        }

        if kb.matches(data, EditorAction::DeleteCharForward) {
            self.delete_char_forward();
            return InputResult::Consumed;
        }

        if kb.matches(data, EditorAction::DeleteWordBackward) {
            self.delete_word_backward();
            return InputResult::Consumed;
        }

        if kb.matches(data, EditorAction::DeleteWordForward) {
            self.delete_word_forward();
            return InputResult::Consumed;
        }

        if kb.matches(data, EditorAction::DeleteToLineEnd) {
            self.kill_to_end();
            return InputResult::Consumed;
        }

        if kb.matches(data, EditorAction::DeleteToLineStart) {
            self.kill_to_start();
            return InputResult::Consumed;
        }

        if kb.matches(data, EditorAction::Yank) {
            self.yank();
            return InputResult::Consumed;
        }

        if kb.matches(data, EditorAction::Undo) {
            self.undo();
            return InputResult::Consumed;
        }

        // Tab → insert spaces
        if kb.matches(data, EditorAction::Tab) {
            self.push_undo();
            let line = &mut self.lines[self.cursor_line];
            line.insert_str(self.cursor_col, "    ");
            self.cursor_col += 4;
            self.dirty = true;
            return InputResult::Consumed;
        }

        // Insert printable characters
        let events = parse_input(data);
        let mut consumed = false;
        for event in events {
            if event.event_type == KeyEventType::Release {
                continue;
            }
            if event.modifiers.is_empty() || event.modifiers == Modifiers::SHIFT {
                if let Key::Char(ch) = event.key {
                    self.insert_char(ch);
                    consumed = true;
                }
            }
        }

        if consumed {
            InputResult::Consumed
        } else {
            InputResult::Ignored
        }
    }

    fn invalidate(&mut self) {
        self.dirty = true;
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }
}

impl Focusable for Editor {
    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        self.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlighted_output_contains_ansi_codes() {
        let mut editor = Editor::new(5).with_language("rs");
        editor.set_value("fn main() {\n    println!(\"hello\");\n}");

        let rendered = editor.render(80);
        let joined = rendered.join("\n");
        assert!(
            joined.contains("\x1b[38;2;"),
            "highlighted output should contain ANSI 24-bit color codes"
        );
        assert!(
            joined.contains("\x1b[0m"),
            "highlighted output should contain ANSI reset"
        );
    }

    #[test]
    fn plain_text_has_no_ansi() {
        let mut editor = Editor::new(3);
        // No language set — should render plain text
        editor.set_value("hello world");

        let rendered = editor.render(40);
        let joined = rendered.join("\n");
        assert!(
            !joined.contains("\x1b[38;2;"),
            "plain text should not have ANSI color codes"
        );
    }

    #[test]
    fn cursor_position_preserved_with_highlighting() {
        let mut editor = Editor::new(3).with_language("rs");
        editor.set_focused(true);
        editor.set_value("let x = 42;");
        // Cursor is at end after set_value — move it to position 0
        editor.cursor_line = 0;
        editor.cursor_col = 0;

        let rendered = editor.render(80);
        let joined = rendered.join("\n");
        assert!(
            joined.contains(CURSOR_MARKER),
            "cursor marker must be present in highlighted output"
        );
    }
}
