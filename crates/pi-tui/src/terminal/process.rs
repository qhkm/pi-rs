use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyModifiers},
    execute, queue,
    style::Print,
    terminal,
};
use std::io::{self, Stdout, Write};
use std::sync::{Arc, Mutex};
use std::thread;

use super::Terminal;

pub struct ProcessTerminal {
    stdout: Stdout,
    columns: u16,
    rows: u16,
    raw_mode: bool,
    kitty_protocol: bool,
    #[allow(dead_code)]
    event_thread: Option<thread::JoinHandle<()>>,
    stop_tx: Option<std::sync::mpsc::Sender<()>>,
}

impl ProcessTerminal {
    pub fn new() -> io::Result<Self> {
        let (columns, rows) = terminal::size()?;
        Ok(Self {
            stdout: io::stdout(),
            columns,
            rows,
            raw_mode: false,
            kitty_protocol: false,
            event_thread: None,
            stop_tx: None,
        })
    }

    fn try_enable_kitty_protocol(&mut self) -> io::Result<()> {
        // CSI > 1 u — enable kitty keyboard protocol (disambiguate escape codes)
        // We try to push the full flags: 31 (all enhancements except report events)
        // If the terminal doesn't support it, the escape is silently ignored.
        self.stdout.write_all(b"\x1b[>31u")?;
        self.stdout.flush()?;
        self.kitty_protocol = true;
        Ok(())
    }

    fn disable_kitty_protocol(&mut self) -> io::Result<()> {
        // CSI < u — pop the kitty keyboard protocol stack
        self.stdout.write_all(b"\x1b[<u")?;
        self.stdout.flush()?;
        self.kitty_protocol = false;
        Ok(())
    }

    fn enable_bracketed_paste(&mut self) -> io::Result<()> {
        // CSI ?2004h
        self.stdout.write_all(b"\x1b[?2004h")?;
        self.stdout.flush()
    }

    fn disable_bracketed_paste(&mut self) -> io::Result<()> {
        // CSI ?2004l
        self.stdout.write_all(b"\x1b[?2004l")?;
        self.stdout.flush()
    }
}

impl Default for ProcessTerminal {
    fn default() -> Self {
        Self::new().expect("Failed to create ProcessTerminal")
    }
}

impl super::Terminal for ProcessTerminal {
    fn start(&mut self, on_resize: Box<dyn Fn() + Send>) -> io::Result<()> {
        self.enable_raw_mode()?;
        self.try_enable_kitty_protocol()?;
        self.enable_bracketed_paste()?;

        let on_resize = Arc::new(Mutex::new(on_resize));
        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
        self.stop_tx = Some(stop_tx);

        let columns = Arc::new(Mutex::new(self.columns));
        let rows = Arc::new(Mutex::new(self.rows));

        let handle = thread::spawn(move || {
            loop {
                // Check for stop signal
                if stop_rx.try_recv().is_ok() {
                    break;
                }

                if event::poll(std::time::Duration::from_millis(100)).unwrap_or(false) {
                    if let Ok(event) = event::read() {
                        match event {
                            Event::Resize(new_cols, new_rows) => {
                                if let Ok(mut c) = columns.lock() {
                                    *c = new_cols;
                                }
                                if let Ok(mut r) = rows.lock() {
                                    *r = new_rows;
                                }
                                if let Ok(cb) = on_resize.lock() {
                                    cb();
                                }
                            }
                            Event::Key(key) => {
                                // Handle Ctrl+C / Ctrl+D as emergency exit
                                if key.code == KeyCode::Char('c')
                                    && key.modifiers.contains(KeyModifiers::CONTROL)
                                {
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        });

        self.event_thread = Some(handle);
        Ok(())
    }

    fn stop(&mut self) -> io::Result<()> {
        // Signal event thread to stop
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }

        self.disable_bracketed_paste()?;
        if self.kitty_protocol {
            self.disable_kitty_protocol()?;
        }
        self.disable_raw_mode()?;
        self.show_cursor()?;

        Ok(())
    }

    fn write(&mut self, data: &str) -> io::Result<()> {
        queue!(self.stdout, Print(data))?;
        self.stdout.flush()
    }

    fn columns(&self) -> u16 {
        self.columns
    }

    fn rows(&self) -> u16 {
        self.rows
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        execute!(self.stdout, cursor::Hide)
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        execute!(self.stdout, cursor::Show)
    }

    fn clear_line(&mut self) -> io::Result<()> {
        execute!(
            self.stdout,
            terminal::Clear(terminal::ClearType::CurrentLine)
        )
    }

    fn clear_from_cursor(&mut self) -> io::Result<()> {
        execute!(
            self.stdout,
            terminal::Clear(terminal::ClearType::FromCursorDown)
        )
    }

    fn clear_screen(&mut self) -> io::Result<()> {
        execute!(self.stdout, terminal::Clear(terminal::ClearType::All))
    }

    fn move_to(&mut self, col: u16, row: u16) -> io::Result<()> {
        execute!(self.stdout, cursor::MoveTo(col, row))
    }

    fn move_by(&mut self, rows: i16) -> io::Result<()> {
        if rows > 0 {
            execute!(self.stdout, cursor::MoveDown(rows as u16))
        } else if rows < 0 {
            execute!(self.stdout, cursor::MoveUp((-rows) as u16))
        } else {
            Ok(())
        }
    }

    fn set_title(&mut self, title: &str) -> io::Result<()> {
        // OSC 2 — set window title
        let seq = format!("\x1b]2;{}\x07", title);
        self.stdout.write_all(seq.as_bytes())?;
        self.stdout.flush()
    }

    fn kitty_protocol_active(&self) -> bool {
        self.kitty_protocol
    }

    fn enable_raw_mode(&mut self) -> io::Result<()> {
        terminal::enable_raw_mode()?;
        self.raw_mode = true;
        Ok(())
    }

    fn disable_raw_mode(&mut self) -> io::Result<()> {
        if self.raw_mode {
            terminal::disable_raw_mode()?;
            self.raw_mode = false;
        }
        Ok(())
    }
}

impl Drop for ProcessTerminal {
    fn drop(&mut self) {
        // Best-effort cleanup
        let _ = self.stop();
    }
}
