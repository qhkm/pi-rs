pub mod process;
pub mod virtual_term;

/// Abstract terminal interface
pub trait Terminal: Send {
    fn start(&mut self, on_resize: Box<dyn Fn() + Send>) -> std::io::Result<()>;
    fn stop(&mut self) -> std::io::Result<()>;
    fn write(&mut self, data: &str) -> std::io::Result<()>;
    fn columns(&self) -> u16;
    fn rows(&self) -> u16;
    fn hide_cursor(&mut self) -> std::io::Result<()>;
    fn show_cursor(&mut self) -> std::io::Result<()>;
    fn clear_line(&mut self) -> std::io::Result<()>;
    fn clear_from_cursor(&mut self) -> std::io::Result<()>;
    fn clear_screen(&mut self) -> std::io::Result<()>;
    fn move_to(&mut self, col: u16, row: u16) -> std::io::Result<()>;
    fn move_by(&mut self, rows: i16) -> std::io::Result<()>;
    fn set_title(&mut self, title: &str) -> std::io::Result<()>;
    fn kitty_protocol_active(&self) -> bool;
    fn enable_raw_mode(&mut self) -> std::io::Result<()>;
    fn disable_raw_mode(&mut self) -> std::io::Result<()>;
}
