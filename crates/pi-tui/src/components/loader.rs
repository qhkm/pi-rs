use crate::components::traits::{Component, InputResult};

const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Animated spinner with a message.
///
/// Call `tick()` periodically (e.g. every 80ms) to advance the animation.
pub struct Loader {
    message: String,
    frame: usize,
    dirty: bool,
}

impl Loader {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            frame: 0,
            dirty: true,
        }
    }

    /// Advance the spinner frame by one step.
    pub fn tick(&mut self) {
        self.frame = (self.frame + 1) % SPINNER_FRAMES.len();
        self.dirty = true;
    }

    /// Update the loader message.
    pub fn set_message(&mut self, message: impl Into<String>) {
        self.message = message.into();
        self.dirty = true;
    }

    /// Get the current spinner frame glyph.
    pub fn current_frame(&self) -> &str {
        SPINNER_FRAMES[self.frame % SPINNER_FRAMES.len()]
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl Default for Loader {
    fn default() -> Self {
        Self::new("")
    }
}

impl Component for Loader {
    fn render(&self, width: u16) -> Vec<String> {
        let spinner = self.current_frame();
        // Format: "⠋ Loading…"
        let line = format!("{} {}", spinner, self.message);

        // Truncate to terminal width
        let mut result = String::new();
        let mut col = 0usize;
        let max_cols = width as usize;
        for ch in line.chars() {
            let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1);
            if col + cw > max_cols {
                break;
            }
            result.push(ch);
            col += cw;
        }

        vec![result]
    }

    fn invalidate(&mut self) {
        self.dirty = true;
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::traits::Component;

    #[test]
    fn test_loader_tick() {
        let mut loader = Loader::new("test");
        assert_eq!(loader.frame, 0);
        loader.tick();
        assert_eq!(loader.frame, 1);
    }

    #[test]
    fn test_loader_render() {
        let loader = Loader::new("Loading");
        let lines = loader.render(80);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("Loading"));
    }
}
