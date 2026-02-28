/// The core component interface. All TUI components implement this.
pub trait Component: Send {
    /// Render the component to lines of text for the given width.
    /// Lines may contain ANSI escape codes.
    fn render(&self, width: u16) -> Vec<String>;

    /// Handle keyboard/terminal input.
    fn handle_input(&mut self, _data: &str) -> InputResult {
        InputResult::Ignored
    }

    /// Whether the component wants key release events (Kitty protocol).
    fn wants_key_release(&self) -> bool {
        false
    }

    /// Mark the component as needing re-render.
    fn invalidate(&mut self);

    /// Whether the component is dirty (needs re-render).
    fn is_dirty(&self) -> bool;
}

/// Result of handling input
#[derive(Debug, Clone, PartialEq)]
pub enum InputResult {
    /// Input was consumed by this component
    Consumed,
    /// Input was not handled by this component
    Ignored,
}

/// Components that can receive focus
pub trait Focusable {
    fn focused(&self) -> bool;
    fn set_focused(&mut self, focused: bool);
}

/// Cursor marker for IME positioning (zero-width APC sequence).
/// This is written at the logical cursor position so the host can
/// position an IME composition window correctly.
pub const CURSOR_MARKER: &str = "\x1b_pi:c\x07";
