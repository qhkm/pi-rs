use std::io;

/// In-memory terminal for testing. Stores written output in a buffer,
/// tracks cursor position, and records all operations.
pub struct VirtualTerminal {
    buffer: Vec<String>,
    columns: u16,
    rows: u16,
    cursor_visible: bool,
    cursor_col: u16,
    cursor_row: u16,
    title: String,
    raw_mode: bool,
}

impl VirtualTerminal {
    pub fn new(columns: u16, rows: u16) -> Self {
        Self {
            buffer: Vec::new(),
            columns,
            rows,
            cursor_visible: true,
            cursor_col: 0,
            cursor_row: 0,
            title: String::new(),
            raw_mode: false,
        }
    }

    /// Get all output lines written to this terminal
    pub fn get_output(&self) -> &[String] {
        &self.buffer
    }

    /// Clear all recorded output
    pub fn clear_output(&mut self) {
        self.buffer.clear();
    }

    pub fn cursor_position(&self) -> (u16, u16) {
        (self.cursor_col, self.cursor_row)
    }

    pub fn is_cursor_visible(&self) -> bool {
        self.cursor_visible
    }

    pub fn title(&self) -> &str {
        &self.title
    }
}

impl super::Terminal for VirtualTerminal {
    fn start(&mut self, _on_resize: Box<dyn Fn() + Send>) -> io::Result<()> {
        // No-op for virtual terminal; no actual I/O
        Ok(())
    }

    fn stop(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn write(&mut self, data: &str) -> io::Result<()> {
        self.buffer.push(data.to_string());
        Ok(())
    }

    fn columns(&self) -> u16 {
        self.columns
    }

    fn rows(&self) -> u16 {
        self.rows
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.cursor_visible = false;
        Ok(())
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.cursor_visible = true;
        Ok(())
    }

    fn clear_line(&mut self) -> io::Result<()> {
        self.buffer.push("<CLEAR_LINE>".to_string());
        Ok(())
    }

    fn clear_from_cursor(&mut self) -> io::Result<()> {
        self.buffer.push("<CLEAR_FROM_CURSOR>".to_string());
        Ok(())
    }

    fn clear_screen(&mut self) -> io::Result<()> {
        self.buffer.push("<CLEAR_SCREEN>".to_string());
        Ok(())
    }

    fn move_to(&mut self, col: u16, row: u16) -> io::Result<()> {
        self.cursor_col = col;
        self.cursor_row = row;
        Ok(())
    }

    fn move_by(&mut self, rows: i16) -> io::Result<()> {
        if rows >= 0 {
            self.cursor_row = self.cursor_row.saturating_add(rows as u16);
        } else {
            self.cursor_row = self.cursor_row.saturating_sub((-rows) as u16);
        }
        Ok(())
    }

    fn set_title(&mut self, title: &str) -> io::Result<()> {
        self.title = title.to_string();
        Ok(())
    }

    fn kitty_protocol_active(&self) -> bool {
        // Virtual terminal reports Kitty protocol as active for testing
        true
    }

    fn enable_raw_mode(&mut self) -> io::Result<()> {
        self.raw_mode = true;
        Ok(())
    }

    fn disable_raw_mode(&mut self) -> io::Result<()> {
        self.raw_mode = false;
        Ok(())
    }
}
