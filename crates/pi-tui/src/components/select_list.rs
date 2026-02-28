use unicode_width::UnicodeWidthStr;
use crate::components::traits::{Component, Focusable, InputResult};
use crate::keyboard::keybindings::{EditorAction, KeybindingsManager};

#[derive(Debug, Clone)]
pub struct SelectItem {
    pub value: String,
    pub label: String,
    pub description: Option<String>,
}

impl SelectItem {
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            description: None,
        }
    }

    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }
}

pub struct SelectListTheme {
    pub selected_prefix: Box<dyn Fn(&str) -> String + Send>,
    pub selected_text: Box<dyn Fn(&str) -> String + Send>,
    pub description: Box<dyn Fn(&str) -> String + Send>,
    pub scroll_info: Box<dyn Fn(&str) -> String + Send>,
    pub no_match: Box<dyn Fn(&str) -> String + Send>,
}

impl Default for SelectListTheme {
    fn default() -> Self {
        Self {
            selected_prefix: Box::new(|s| format!("\x1b[36m{}\x1b[0m", s)),
            selected_text: Box::new(|s| format!("\x1b[1;37m{}\x1b[0m", s)),
            description: Box::new(|s| format!("\x1b[90m{}\x1b[0m", s)),
            scroll_info: Box::new(|s| format!("\x1b[90m{}\x1b[0m", s)),
            no_match: Box::new(|s| format!("\x1b[90m{}\x1b[0m", s)),
        }
    }
}

pub struct SelectList {
    items: Vec<SelectItem>,
    filtered_items: Vec<usize>,
    selected_index: usize,
    max_visible: usize,
    scroll_offset: usize,
    filter: String,
    focused: bool,
    dirty: bool,
    theme: SelectListTheme,
    keybindings: KeybindingsManager,
    pub on_select: Option<Box<dyn Fn(&SelectItem) + Send>>,
    pub on_cancel: Option<Box<dyn Fn() + Send>>,
}

impl SelectList {
    pub fn new(items: Vec<SelectItem>, max_visible: usize) -> Self {
        let filtered_items: Vec<usize> = (0..items.len()).collect();
        Self {
            items,
            filtered_items,
            selected_index: 0,
            max_visible,
            scroll_offset: 0,
            filter: String::new(),
            focused: false,
            dirty: true,
            theme: SelectListTheme::default(),
            keybindings: KeybindingsManager::new(),
            on_select: None,
            on_cancel: None,
        }
    }

    pub fn with_theme(mut self, theme: SelectListTheme) -> Self {
        self.theme = theme;
        self
    }

    pub fn filter(&self) -> &str {
        &self.filter
    }

    pub fn set_filter(&mut self, filter: impl Into<String>) {
        self.filter = filter.into();
        self.apply_filter();
        self.dirty = true;
    }

    pub fn selected_item(&self) -> Option<&SelectItem> {
        self.filtered_items
            .get(self.selected_index)
            .and_then(|&idx| self.items.get(idx))
    }

    pub fn set_items(&mut self, items: Vec<SelectItem>) {
        self.items = items;
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.apply_filter();
        self.dirty = true;
    }

    fn apply_filter(&mut self) {
        if self.filter.is_empty() {
            self.filtered_items = (0..self.items.len()).collect();
        } else {
            let f = self.filter.to_lowercase();
            self.filtered_items = self
                .items
                .iter()
                .enumerate()
                .filter(|(_, item)| {
                    item.label.to_lowercase().contains(&f)
                        || item.value.to_lowercase().contains(&f)
                })
                .map(|(i, _)| i)
                .collect();
        }
        // Clamp selected index
        if self.selected_index >= self.filtered_items.len() {
            self.selected_index = self.filtered_items.len().saturating_sub(1);
        }
        self.ensure_scroll();
    }

    fn ensure_scroll(&mut self) {
        if self.filtered_items.is_empty() { return; }
        if self.selected_index < self.scroll_offset {
            self.scroll_offset = self.selected_index;
        }
        if self.selected_index >= self.scroll_offset + self.max_visible {
            self.scroll_offset = self.selected_index - self.max_visible + 1;
        }
    }

    fn move_up(&mut self) {
        let len = self.filtered_items.len();
        if len == 0 { return; }
        if self.selected_index == 0 {
            self.selected_index = len - 1;
        } else {
            self.selected_index -= 1;
        }
        self.ensure_scroll();
        self.dirty = true;
    }

    fn move_down(&mut self) {
        let len = self.filtered_items.len();
        if len == 0 { return; }
        self.selected_index = (self.selected_index + 1) % len;
        self.ensure_scroll();
        self.dirty = true;
    }
}

impl Component for SelectList {
    fn render(&self, width: u16) -> Vec<String> {
        let mut lines = Vec::new();
        let w = width as usize;

        if self.filtered_items.is_empty() {
            let msg = (self.theme.no_match)("No matches");
            lines.push(msg);
            return lines;
        }

        let visible_count = self.max_visible.min(self.filtered_items.len());
        let end = (self.scroll_offset + visible_count).min(self.filtered_items.len());

        for vis_idx in self.scroll_offset..end {
            let item_idx = self.filtered_items[vis_idx];
            let item = &self.items[item_idx];
            let is_selected = vis_idx == self.selected_index;

            let label_display = if w > 4 {
                truncate_visible(&item.label, w.saturating_sub(4))
            } else {
                item.label.clone()
            };

            let line = if is_selected {
                let prefix = (self.theme.selected_prefix)("> ");
                let text = (self.theme.selected_text)(&label_display);
                format!("{}{}", prefix, text)
            } else {
                format!("  {}", label_display)
            };

            lines.push(line);

            // Show description for selected item
            if is_selected {
                if let Some(ref desc) = item.description {
                    let d = format!("    {}", desc);
                    let d_trunc = truncate_visible(&d, w);
                    lines.push((self.theme.description)(&d_trunc));
                }
            }
        }

        // Scroll indicator
        let total = self.filtered_items.len();
        if total > self.max_visible {
            let info = format!(
                "{}/{} (↑↓)",
                self.selected_index + 1,
                total
            );
            lines.push((self.theme.scroll_info)(&info));
        }

        lines
    }

    fn handle_input(&mut self, data: &str) -> InputResult {
        if !self.focused {
            return InputResult::Ignored;
        }

        let kb = &self.keybindings;

        if kb.matches(data, EditorAction::SelectUp) || kb.matches(data, EditorAction::CursorUp) {
            self.move_up();
            return InputResult::Consumed;
        }

        if kb.matches(data, EditorAction::SelectDown) || kb.matches(data, EditorAction::CursorDown) {
            self.move_down();
            return InputResult::Consumed;
        }

        if kb.matches(data, EditorAction::SelectConfirm) {
            if let Some(item) = self.selected_item().cloned() {
                if let Some(ref cb) = self.on_select {
                    cb(&item);
                }
            }
            return InputResult::Consumed;
        }

        if kb.matches(data, EditorAction::SelectCancel) || data == "\x1b" {
            if let Some(ref cb) = self.on_cancel {
                cb();
            }
            return InputResult::Consumed;
        }

        InputResult::Ignored
    }

    fn invalidate(&mut self) {
        self.dirty = true;
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }
}

impl Focusable for SelectList {
    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        self.dirty = true;
    }
}

fn truncate_visible(s: &str, max_width: usize) -> String {
    let mut result = String::new();
    let mut width = 0;
    for ch in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1);
        if width + cw > max_width { break; }
        result.push(ch);
        width += cw;
    }
    result
}
