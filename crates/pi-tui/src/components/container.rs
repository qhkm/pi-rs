use crate::components::traits::{Component, InputResult};

/// Simple container that renders children vertically, stacking them top-to-bottom.
pub struct Container {
    children: Vec<Box<dyn Component>>,
    dirty: bool,
}

impl Container {
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
            dirty: true,
        }
    }

    pub fn add_child(&mut self, child: Box<dyn Component>) {
        self.children.push(child);
        self.dirty = true;
    }

    pub fn children(&self) -> &[Box<dyn Component>] {
        &self.children
    }

    pub fn children_mut(&mut self) -> &mut [Box<dyn Component>] {
        &mut self.children
    }
}

impl Default for Container {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for Container {
    fn render(&self, width: u16) -> Vec<String> {
        let mut lines = Vec::new();
        for child in &self.children {
            lines.extend(child.render(width));
        }
        lines
    }

    fn handle_input(&mut self, data: &str) -> InputResult {
        for child in &mut self.children {
            if child.handle_input(data) == InputResult::Consumed {
                self.dirty = true;
                return InputResult::Consumed;
            }
        }
        InputResult::Ignored
    }

    fn invalidate(&mut self) {
        self.dirty = true;
        for child in &mut self.children {
            child.invalidate();
        }
    }

    fn is_dirty(&self) -> bool {
        self.dirty || self.children.iter().any(|c| c.is_dirty())
    }
}

// ============================================================================
// TuiBox — adds padding and optional background to a single child
// ============================================================================

/// Box adds padding and an optional background color to its child component.
pub struct TuiBox {
    child: Option<Box<dyn Component>>,
    padding_x: u16,
    padding_y: u16,
    bg_fn: Option<Box<dyn Fn(&str) -> String + Send>>,
    dirty: bool,
}

impl TuiBox {
    pub fn new(padding_x: u16, padding_y: u16) -> Self {
        Self {
            child: None,
            padding_x,
            padding_y,
            bg_fn: None,
            dirty: true,
        }
    }

    pub fn with_background(mut self, bg_fn: Box<dyn Fn(&str) -> String + Send>) -> Self {
        self.bg_fn = Some(bg_fn);
        self
    }

    pub fn set_child(&mut self, child: Box<dyn Component>) {
        self.child = Some(child);
        self.dirty = true;
    }
}

impl Default for TuiBox {
    fn default() -> Self {
        Self::new(0, 0)
    }
}

impl Component for TuiBox {
    fn render(&self, width: u16) -> Vec<String> {
        let inner_width = (width as i32 - 2 * self.padding_x as i32).max(0) as u16;
        let pad_x = " ".repeat(self.padding_x as usize);

        let mut child_lines = match &self.child {
            Some(c) => c.render(inner_width),
            None => Vec::new(),
        };

        let mut result = Vec::new();

        // Top padding
        for _ in 0..self.padding_y {
            let blank = " ".repeat(width as usize);
            result.push(self.apply_bg(&blank));
        }

        // Child lines with horizontal padding
        for line in &child_lines {
            let padded = format!("{}{}{}", pad_x, line, pad_x);
            result.push(self.apply_bg(&padded));
        }

        // Bottom padding
        for _ in 0..self.padding_y {
            let blank = " ".repeat(width as usize);
            result.push(self.apply_bg(&blank));
        }

        result
    }

    fn handle_input(&mut self, data: &str) -> InputResult {
        if let Some(ref mut child) = self.child {
            let result = child.handle_input(data);
            if result == InputResult::Consumed {
                self.dirty = true;
            }
            result
        } else {
            InputResult::Ignored
        }
    }

    fn invalidate(&mut self) {
        self.dirty = true;
        if let Some(ref mut child) = self.child {
            child.invalidate();
        }
    }

    fn is_dirty(&self) -> bool {
        self.dirty || self.child.as_ref().map(|c| c.is_dirty()).unwrap_or(false)
    }
}

impl TuiBox {
    fn apply_bg(&self, s: &str) -> String {
        match &self.bg_fn {
            Some(f) => f(s),
            None => s.to_string(),
        }
    }
}

// ============================================================================
// Spacer — vertical whitespace
// ============================================================================

/// Vertical spacer — renders N empty lines.
pub struct Spacer {
    lines: u16,
}

impl Spacer {
    pub fn new(lines: u16) -> Self {
        Self { lines }
    }
}

impl Component for Spacer {
    fn render(&self, _width: u16) -> Vec<String> {
        (0..self.lines).map(|_| String::new()).collect()
    }

    fn invalidate(&mut self) {
        // Spacer never needs re-render
    }

    fn is_dirty(&self) -> bool {
        false
    }
}
