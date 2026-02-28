/// Begin synchronized output — CSI ?2026h.
///
/// While synchronized output is active, the terminal defers screen updates
/// until `end_sync` is called, preventing partial-render flicker.
pub fn begin_sync(terminal: &mut dyn crate::terminal::Terminal) -> std::io::Result<()> {
    terminal.write("\x1b[?2026h")
}

/// End synchronized output — CSI ?2026l.
pub fn end_sync(terminal: &mut dyn crate::terminal::Terminal) -> std::io::Result<()> {
    terminal.write("\x1b[?2026l")
}
