use unicode_width::UnicodeWidthStr;
use crate::components::traits::{Component, Focusable, InputResult, CURSOR_MARKER};
use crate::keyboard::kitty::parse_input;
use crate::keyboard::keybindings::{EditorAction, KeybindingsManager};

/// Single-line text input component.
///
/// Features:
/// - Cursor movement (left/right, home/end, word-left/word-right)
/// - Text editing (insert, delete, backspace, word-delete)
/// - Kill ring (Ctrl+K, Ctrl+U, Ctrl+Y, Alt+Y)
/// - Undo (Ctrl+Z)
/// - Horizontal scrolling when text exceeds width
/// - Bracketed paste handling
/// - Submit (Enter) and Escape callbacks
pub struct Input {
    value: String,
    cursor: usize,
    scroll_offset: usize,
    focused: bool,
    dirty: bool,
    kill_ring: Vec<String>,
    kill_ring_index: usize,
    undo_stack: Vec<(String, usize)>,
    keybindings: KeybindingsManager,
    in_bracketed_paste: bool,
    pub on_submit: Option<Box<dyn Fn(&str) + Send>>,
    pub on_escape: Option<Box<dyn Fn() + Send>>,
}

impl Input {
    pub fn new() -> Self {
        Self {
            value: String::new(),
            cursor: 0,
            scroll_offset: 0,
            focused: false,
            dirty: true,
            kill_ring: Vec::new(),
            kill_ring_index: 0,
            undo_stack: Vec::new(),
            keybindings: KeybindingsManager::new(),
            in_bracketed_paste: false,
            on_submit: None,
            on_escape: None,
        }
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn set_value(&mut self, value: impl Into<String>) {
        let s = value.into();
        self.cursor = s.len();
        self.value = s;
        self.scroll_offset = 0;
        self.dirty = true;
    }

    pub fn clear(&mut self) {
        self.push_undo();
        self.value.clear();
        self.cursor = 0;
        self.scroll_offset = 0;
        self.dirty = true;
    }

    // -----------------------------------------------------------------------
    // Cursor helpers
    // -----------------------------------------------------------------------

    fn push_undo(&mut self) {
        self.undo_stack.push((self.value.clone(), self.cursor));
        // Keep undo stack from growing unbounded
        if self.undo_stack.len() > 200 {
            self.undo_stack.remove(0);
        }
    }

    fn char_boundary_left(&self, pos: usize) -> usize {
        if pos == 0 {
            return 0;
        }
        let mut p = pos - 1;
        while !self.value.is_char_boundary(p) {
            p -= 1;
        }
        p
    }

    fn char_boundary_right(&self, pos: usize) -> usize {
        if pos >= self.value.len() {
            return self.value.len();
        }
        let mut p = pos + 1;
        while !self.value.is_char_boundary(p) {
            p += 1;
        }
        p
    }

    fn word_left(&self, pos: usize) -> usize {
        let chars: Vec<char> = self.value[..pos].chars().collect();
        let mut i = chars.len();

        // Skip trailing spaces
        while i > 0 && chars[i - 1] == ' ' {
            i -= 1;
        }
        // Skip word chars
        while i > 0 && chars[i - 1] != ' ' {
            i -= 1;
        }

        self.value[..pos]
            .char_indices()
            .nth(i)
            .map(|(idx, _)| idx)
            .unwrap_or(0)
    }

    fn word_right(&self, pos: usize) -> usize {
        let s = &self.value[pos..];
        let mut chars = s.char_indices();

        // Skip leading spaces
        let mut byte_offset = 0;
        for (idx, ch) in chars.by_ref() {
            if ch != ' ' {
                byte_offset = idx;
                break;
            }
            byte_offset = idx + ch.len_utf8();
        }

        // Skip word chars
        for (idx, ch) in s.char_indices() {
            if idx < byte_offset {
                continue;
            }
            if ch == ' ' {
                return pos + idx;
            }
        }

        self.value.len()
    }

    fn update_scroll(&mut self, visible_width: usize) {
        // Keep cursor visible within the scroll window
        let cursor_display_pos = UnicodeWidthStr::width(&self.value[self.scroll_offset..self.cursor]);

        if cursor_display_pos >= visible_width {
            // Scroll right
            let excess = cursor_display_pos - visible_width + 1;
            let mut advance = 0;
            let mut byte_advance = 0;
            for ch in self.value[self.scroll_offset..].chars() {
                let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1);
                if advance + w > excess {
                    break;
                }
                advance += w;
                byte_advance += ch.len_utf8();
            }
            self.scroll_offset += byte_advance;
        } else if self.cursor < self.scroll_offset {
            self.scroll_offset = self.cursor;
        }

        // Ensure scroll_offset is a valid char boundary
        while self.scroll_offset > 0 && !self.value.is_char_boundary(self.scroll_offset) {
            self.scroll_offset -= 1;
        }
    }

    // -----------------------------------------------------------------------
    // Editing operations
    // -----------------------------------------------------------------------

    fn insert_str(&mut self, s: &str) {
        self.push_undo();
        self.value.insert_str(self.cursor, s);
        self.cursor += s.len();
        self.dirty = true;
    }

    fn delete_char_backward(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.push_undo();
        let prev = self.char_boundary_left(self.cursor);
        self.value.drain(prev..self.cursor);
        self.cursor = prev;
        if self.scroll_offset > self.cursor {
            self.scroll_offset = self.cursor;
        }
        self.dirty = true;
    }

    fn delete_char_forward(&mut self) {
        if self.cursor >= self.value.len() {
            return;
        }
        self.push_undo();
        let next = self.char_boundary_right(self.cursor);
        self.value.drain(self.cursor..next);
        self.dirty = true;
    }

    fn delete_word_backward(&mut self) {
        let new_pos = self.word_left(self.cursor);
        if new_pos == self.cursor {
            return;
        }
        self.push_undo();
        let deleted = self.value[new_pos..self.cursor].to_string();
        self.value.drain(new_pos..self.cursor);
        self.cursor = new_pos;
        self.push_to_kill_ring(deleted);
        self.dirty = true;
    }

    fn delete_word_forward(&mut self) {
        let new_pos = self.word_right(self.cursor);
        if new_pos == self.cursor {
            return;
        }
        self.push_undo();
        let deleted = self.value[self.cursor..new_pos].to_string();
        self.value.drain(self.cursor..new_pos);
        self.push_to_kill_ring(deleted);
        self.dirty = true;
    }

    fn kill_to_end(&mut self) {
        if self.cursor >= self.value.len() {
            return;
        }
        self.push_undo();
        let killed = self.value[self.cursor..].to_string();
        self.value.truncate(self.cursor);
        self.push_to_kill_ring(killed);
        self.dirty = true;
    }

    fn kill_to_start(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.push_undo();
        let killed = self.value[..self.cursor].to_string();
        self.value.drain(..self.cursor);
        self.cursor = 0;
        self.scroll_offset = 0;
        self.push_to_kill_ring(killed);
        self.dirty = true;
    }

    fn push_to_kill_ring(&mut self, text: String) {
        self.kill_ring.push(text);
        self.kill_ring_index = self.kill_ring.len().saturating_sub(1);
    }

    fn yank(&mut self) {
        if self.kill_ring.is_empty() {
            return;
        }
        let text = self.kill_ring[self.kill_ring_index].clone();
        self.insert_str(&text);
    }

    fn yank_pop(&mut self) {
        if self.kill_ring.is_empty() {
            return;
        }
        if self.kill_ring_index == 0 {
            self.kill_ring_index = self.kill_ring.len() - 1;
        } else {
            self.kill_ring_index -= 1;
        }
        // TODO: replace the last yank with the new kill ring entry
        // For now just yank the new entry
        let text = self.kill_ring[self.kill_ring_index].clone();
        self.insert_str(&text);
    }

    fn undo(&mut self) {
        if let Some((value, cursor)) = self.undo_stack.pop() {
            self.value = value;
            self.cursor = cursor;
            self.scroll_offset = 0;
            self.dirty = true;
        }
    }

    // -----------------------------------------------------------------------
    // Handle bracketed paste
    // -----------------------------------------------------------------------

    fn handle_bracketed_paste_start(&mut self, remaining: &str) -> (&'static str, usize) {
        // Find \x1b[201~
        const END_MARKER: &str = "\x1b[201~";
        if let Some(end_pos) = remaining.find(END_MARKER) {
            let paste_text = &remaining[..end_pos];
            self.insert_str(paste_text);
            return ("", end_pos + END_MARKER.len());
        }
        // Paste without end marker — insert everything
        self.insert_str(remaining);
        ("", remaining.len())
    }
}

impl Default for Input {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for Input {
    fn render(&self, width: u16) -> Vec<String> {
        if width == 0 {
            return vec![String::new()];
        }

        let visible_width = width as usize;

        // Build the visible slice of the value
        let visible = &self.value[self.scroll_offset..];
        let mut display = String::new();
        let mut display_width = 0usize;
        let mut cursor_col: Option<usize> = None;

        // Compute cursor relative to scroll offset
        let cursor_rel = if self.cursor >= self.scroll_offset {
            self.cursor - self.scroll_offset
        } else {
            0
        };

        let mut byte_pos = 0;
        for ch in visible.chars() {
            let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1);

            if self.focused && cursor_col.is_none() && byte_pos >= cursor_rel {
                cursor_col = Some(display_width);
                if self.focused {
                    display.push_str(CURSOR_MARKER);
                }
            }

            if display_width + cw > visible_width {
                break;
            }

            display.push(ch);
            display_width += cw;
            byte_pos += ch.len_utf8();
        }

        // Cursor at end of text
        if self.focused && cursor_col.is_none() {
            display.push_str(CURSOR_MARKER);
        }

        // Pad to fill width
        while display_width < visible_width {
            display.push(' ');
            display_width += 1;
        }

        vec![display]
    }

    fn handle_input(&mut self, data: &str) -> InputResult {
        if !self.focused {
            return InputResult::Ignored;
        }

        // Bracketed paste start
        if data.starts_with("\x1b[200~") {
            let rest = &data[6..];
            self.handle_bracketed_paste_start(rest);
            self.dirty = true;
            return InputResult::Consumed;
        }

        let kb = &self.keybindings;

        // Check keybindings
        if kb.matches(data, EditorAction::Submit) {
            if let Some(ref cb) = self.on_submit {
                let v = self.value.clone();
                cb(&v);
            }
            return InputResult::Consumed;
        }

        if data == "\x1b" {
            // Escape
            if let Some(ref cb) = self.on_escape {
                cb();
            }
            return InputResult::Consumed;
        }

        if kb.matches(data, EditorAction::CursorLeft) {
            let prev = self.char_boundary_left(self.cursor);
            self.cursor = prev;
            self.dirty = true;
            return InputResult::Consumed;
        }

        if kb.matches(data, EditorAction::CursorRight) {
            let next = self.char_boundary_right(self.cursor);
            self.cursor = next;
            self.dirty = true;
            return InputResult::Consumed;
        }

        if kb.matches(data, EditorAction::CursorWordLeft) {
            self.cursor = self.word_left(self.cursor);
            self.dirty = true;
            return InputResult::Consumed;
        }

        if kb.matches(data, EditorAction::CursorWordRight) {
            self.cursor = self.word_right(self.cursor);
            self.dirty = true;
            return InputResult::Consumed;
        }

        if kb.matches(data, EditorAction::CursorLineStart) {
            self.cursor = 0;
            self.scroll_offset = 0;
            self.dirty = true;
            return InputResult::Consumed;
        }

        if kb.matches(data, EditorAction::CursorLineEnd) {
            self.cursor = self.value.len();
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

        if kb.matches(data, EditorAction::YankPop) {
            self.yank_pop();
            return InputResult::Consumed;
        }

        if kb.matches(data, EditorAction::Undo) {
            self.undo();
            return InputResult::Consumed;
        }

        // Insert printable characters
        let events = parse_input(data);
        for event in events {
            use crate::keyboard::kitty::{Key, KeyEventType, Modifiers};
            if event.event_type == KeyEventType::Release {
                continue;
            }
            if event.modifiers.is_empty() || event.modifiers == Modifiers::SHIFT {
                if let Key::Char(ch) = event.key {
                    let mut buf = [0u8; 4];
                    let s = ch.encode_utf8(&mut buf);
                    self.insert_str(s);
                }
            }
        }

        self.dirty = true;
        InputResult::Consumed
    }

    fn invalidate(&mut self) {
        self.dirty = true;
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }
}

impl Focusable for Input {
    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        self.dirty = true;
    }
}
