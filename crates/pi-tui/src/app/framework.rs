/// High-level TUI application framework.
///
/// Provides an opinionated event loop and layout system for building
/// interactive terminal applications. The framework handles:
/// - Terminal initialization and cleanup
/// - Event loop with async support
/// - Layout management (header, body, footer, input)
/// - Keyboard input dispatch
/// - Component lifecycle
use std::io::{self, stdout};

use crossterm::{
    cursor,
    event::{self, Event, KeyEvent},
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};

use crate::components::Component;
use crate::rendering::DifferentialRenderer;
use crate::terminal::process::ProcessTerminal;
use crate::terminal::Terminal;

/// Result of handling an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppResult {
    /// Continue running
    Continue,
    /// Exit the application
    Exit(i32),
    /// Event was handled, don't propagate
    Handled,
}

/// Context available during app lifecycle callbacks.
pub struct AppContext {
    /// The terminal for direct output
    pub term: ProcessTerminal,
    /// The differential renderer
    pub renderer: DifferentialRenderer,
    /// Current terminal size
    pub size: (u16, u16),
    /// Whether the app should exit
    pub should_exit: bool,
    /// Exit code
    pub exit_code: i32,
    /// Current focus area
    pub focus: FocusArea,
}

/// Focus areas in the application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusArea {
    #[default]
    /// Main content area
    Main,
    /// Input area
    Input,
    /// Command palette
    CommandPalette,
    /// Status bar / footer
    Footer,
}

impl AppContext {
    /// Create a new app context with the given terminal size.
    pub fn new(width: u16, height: u16) -> io::Result<Self> {
        Ok(Self {
            term: ProcessTerminal::new()?,
            renderer: DifferentialRenderer::new(),
            size: (width, height),
            should_exit: false,
            exit_code: 0,
            focus: FocusArea::Input,
        })
    }

    /// Request exit with the given code.
    pub fn exit(&mut self, code: i32) {
        self.should_exit = true;
        self.exit_code = code;
    }

    /// Update the terminal size.
    pub fn resize(&mut self, width: u16, height: u16) {
        self.size = (width, height);
        self.renderer.invalidate();
    }
}

/// A running TUI application.
///
/// This is the main entry point for building TUI apps. Create an app,
/// configure it with handlers, and run it.
pub struct App<T> {
    /// User state
    state: T,
    /// Keyboard event handler
    key_handler: Box<dyn FnMut(&mut T, KeyEvent, &mut AppContext) -> AppResult>,
    /// Render handler
    render_handler: Box<dyn FnMut(&T, &mut AppContext)>,
    /// Tick handler (optional)
    tick_handler: Option<Box<dyn FnMut(&mut T, &mut AppContext) -> AppResult>>,
    /// Tick interval (default: 100ms)
    tick_interval: std::time::Duration,
    /// Whether to use alternate screen
    use_alternate_screen: bool,
    /// Whether to enable raw mode
    enable_raw_mode: bool,
}

impl<T> App<T> {
    /// Create a new app with the given state and handlers.
    pub fn new(
        state: T,
        key_handler: impl FnMut(&mut T, KeyEvent, &mut AppContext) -> AppResult + 'static,
        render_handler: impl FnMut(&T, &mut AppContext) + 'static,
    ) -> Self {
        Self {
            state,
            key_handler: Box::new(key_handler),
            render_handler: Box::new(render_handler),
            tick_handler: None,
            tick_interval: std::time::Duration::from_millis(100),
            use_alternate_screen: true,
            enable_raw_mode: true,
        }
    }

    /// Set the tick handler and interval.
    pub fn on_tick(
        mut self,
        interval: std::time::Duration,
        handler: impl FnMut(&mut T, &mut AppContext) -> AppResult + 'static,
    ) -> Self {
        self.tick_handler = Some(Box::new(handler));
        self.tick_interval = interval;
        self
    }

    /// Disable alternate screen (useful for inline apps).
    pub fn no_alternate_screen(mut self) -> Self {
        self.use_alternate_screen = false;
        self
    }

    /// Disable raw mode (useful for testing).
    pub fn no_raw_mode(mut self) -> Self {
        self.enable_raw_mode = false;
        self
    }

    /// Run the application.
    pub fn run(mut self) -> io::Result<i32> {
        // Setup terminal
        if self.enable_raw_mode {
            terminal::enable_raw_mode()?;
        }

        let mut stdout = stdout();

        if self.use_alternate_screen {
            stdout.execute(EnterAlternateScreen)?;
        }

        stdout.execute(cursor::Hide)?;
        stdout.execute(Clear(ClearType::All))?;

        // Get initial size
        let (cols, rows) = terminal::size()?;
        let mut ctx = AppContext::new(cols, rows)?;

        // Main event loop
        let result = self.run_loop(&mut ctx);

        // Cleanup
        stdout.execute(cursor::Show)?;

        if self.use_alternate_screen {
            stdout.execute(LeaveAlternateScreen)?;
        }

        if self.enable_raw_mode {
            terminal::disable_raw_mode()?;
        }

        match result {
            Ok(_) => Ok(ctx.exit_code),
            Err(e) => Err(e),
        }
    }

    fn run_loop(&mut self, ctx: &mut AppContext) -> io::Result<()> {
        let mut last_tick = std::time::Instant::now();

        loop {
            // Handle events
            if event::poll(std::time::Duration::from_millis(10))? {
                match event::read()? {
                    Event::Key(key) => {
                        let result = (self.key_handler)(&mut self.state, key, ctx);
                        match result {
                            AppResult::Exit(code) => {
                                ctx.exit(code);
                                break;
                            }
                            AppResult::Continue => {}
                            AppResult::Handled => continue,
                        }
                    }
                    Event::Resize(cols, rows) => {
                        ctx.resize(cols, rows);
                    }
                    _ => {}
                }
            }

            // Handle tick
            let now = std::time::Instant::now();
            if now.duration_since(last_tick) >= self.tick_interval {
                if let Some(ref mut tick_handler) = self.tick_handler {
                    let result = (tick_handler)(&mut self.state, ctx);
                    match result {
                        AppResult::Exit(code) => {
                            ctx.exit(code);
                            break;
                        }
                        AppResult::Continue | AppResult::Handled => {}
                    }
                }
                last_tick = now;
            }

            if ctx.should_exit {
                break;
            }

            // Render
            (self.render_handler)(&self.state, ctx);
        }

        Ok(())
    }
}

/// A layout-based app with predefined regions.
///
/// Provides a higher-level abstraction with built-in layout management
/// for common patterns like chat apps with an input area and status bar.
pub struct LayoutApp<T> {
    state: T,
    header_height: u16,
    footer_height: u16,
    input_height: u16,
    key_handler: Box<dyn FnMut(&mut T, KeyEvent, &mut AppContext) -> AppResult>,
    render_header: Box<dyn FnMut(&T, &mut AppContext)>,
    render_body: Box<dyn FnMut(&T, &mut AppContext)>,
    render_footer: Box<dyn FnMut(&T, &mut AppContext)>,
    render_input: Box<dyn FnMut(&T, &mut AppContext)>,
}

impl<T> LayoutApp<T> {
    /// Create a new layout app.
    pub fn new(
        state: T,
        key_handler: impl FnMut(&mut T, KeyEvent, &mut AppContext) -> AppResult + 'static,
        render_body: impl FnMut(&T, &mut AppContext) + 'static,
    ) -> Self {
        Self {
            state,
            header_height: 0,
            footer_height: 1,
            input_height: 3,
            key_handler: Box::new(key_handler),
            render_header: Box::new(|_, _| {}),
            render_body: Box::new(render_body),
            render_footer: Box::new(|_, ctx| {
                // Default footer
                let (_, height) = ctx.size;
                let _ = ctx.term.move_to(0, height - 1);
                let _ = ctx.term.write("Press Ctrl+C to exit");
            }),
            render_input: Box::new(|_, ctx| {
                // Default input area
                let (width, height) = ctx.size;
                let y = height - 2;
                let line: String = std::iter::repeat('─').take(width as usize).collect();
                let _ = ctx.term.move_to(0, y - 1);
                let _ = ctx.term.write(&line);
            }),
        }
    }

    /// Set the header height and renderer.
    pub fn with_header(
        mut self,
        height: u16,
        renderer: impl FnMut(&T, &mut AppContext) + 'static,
    ) -> Self {
        self.header_height = height;
        self.render_header = Box::new(renderer);
        self
    }

    /// Set the footer height and renderer.
    pub fn with_footer(
        mut self,
        height: u16,
        renderer: impl FnMut(&T, &mut AppContext) + 'static,
    ) -> Self {
        self.footer_height = height;
        self.render_footer = Box::new(renderer);
        self
    }

    /// Set the input area height and renderer.
    pub fn with_input(
        mut self,
        height: u16,
        renderer: impl FnMut(&T, &mut AppContext) + 'static,
    ) -> Self {
        self.input_height = height;
        self.render_input = Box::new(renderer);
        self
    }

    /// Get the body area dimensions.
    pub fn body_area(&self, ctx: &AppContext) -> (u16, u16, u16, u16) {
        let (width, height) = ctx.size;
        let top = self.header_height;
        let bottom = height.saturating_sub(self.footer_height + self.input_height);
        let body_height = bottom.saturating_sub(top);
        (0, top, width, body_height)
    }

    /// Run the application.
    pub fn run(mut self) -> io::Result<i32> {
        terminal::enable_raw_mode()?;

        let mut stdout = stdout();
        stdout.execute(EnterAlternateScreen)?;
        stdout.execute(cursor::Hide)?;
        stdout.execute(Clear(ClearType::All))?;

        let (cols, rows) = terminal::size()?;
        let mut ctx = AppContext::new(cols, rows)?;

        let result = self.run_loop(&mut ctx);

        stdout.execute(cursor::Show)?;
        stdout.execute(LeaveAlternateScreen)?;
        terminal::disable_raw_mode()?;

        match result {
            Ok(_) => Ok(ctx.exit_code),
            Err(e) => Err(e),
        }
    }

    fn run_loop(&mut self, ctx: &mut AppContext) -> io::Result<()> {
        loop {
            if event::poll(std::time::Duration::from_millis(10))? {
                match event::read()? {
                    Event::Key(key) => {
                        let result = (self.key_handler)(&mut self.state, key, ctx);
                        match result {
                            AppResult::Exit(code) => {
                                ctx.exit(code);
                                break;
                            }
                            AppResult::Continue => {}
                            AppResult::Handled => continue,
                        }
                    }
                    Event::Resize(cols, rows) => {
                        ctx.resize(cols, rows);
                    }
                    _ => {}
                }
            }

            if ctx.should_exit {
                break;
            }

            // Render layout
            self.render(ctx)?;
        }

        Ok(())
    }

    fn render(&mut self, ctx: &mut AppContext) -> io::Result<()> {
        let (width, height) = ctx.size;

        // Clear
        ctx.term.clear_screen()?;

        // Render header
        if self.header_height > 0 {
            (self.render_header)(&self.state, ctx);
        }

        // Render body
        (self.render_body)(&self.state, ctx);

        // Render footer
        if self.footer_height > 0 {
            let footer_y = height - self.footer_height - self.input_height;
            ctx.term.move_to(0, footer_y)?;
            (self.render_footer)(&self.state, ctx);
        }

        // Render input area
        if self.input_height > 0 {
            let input_y = height - self.input_height;
            ctx.term.move_to(0, input_y)?;
            (self.render_input)(&self.state, ctx);
        }


        Ok(())
    }
}

/// Utility functions for common rendering tasks.
pub mod render {
    use super::*;

    /// Render a status bar with the given items.
    pub fn status_bar(ctx: &mut AppContext, y: u16, items: &[(&str, &str)]) {
        let (width, _) = ctx.size;
        let mut x = 0u16;
        let separator = " │ ";

        for (i, (label, value)) in items.iter().enumerate() {
            let text = if i < items.len() - 1 {
                format!("{}: {}{}", label, value, separator)
            } else {
                format!("{}: {}", label, value)
            };

            let text_len = text.len() as u16;
            if x + text_len > width {
                break;
            }

            let _ = ctx.term.move_to(x, y);
            let _ = ctx.term.write(&text);
            x += text_len;
        }

        // Fill rest with spaces
        if x < width {
            let spaces = " ".repeat((width - x) as usize);
            let _ = ctx.term.move_to(x, y);
            let _ = ctx.term.write(&spaces);
        }
    }

    /// Render a horizontal line.
    pub fn hline(ctx: &mut AppContext, y: u16, ch: char) {
        let (width, _) = ctx.size;
        let line: String = std::iter::repeat(ch).take(width as usize).collect();
        let _ = ctx.term.move_to(0, y);
        let _ = ctx.term.write(&line);
    }

    /// Render a box border.
    pub fn box_border(ctx: &mut AppContext, x: u16, y: u16, w: u16, h: u16) {
        let _ = ctx.term.move_to(x, y);
        let _ = ctx.term.write("┌");
        let _ = ctx.term.move_to(x + w - 1, y);
        let _ = ctx.term.write("┐");
        let _ = ctx.term.move_to(x, y + h - 1);
        let _ = ctx.term.write("└");
        let _ = ctx.term.move_to(x + w - 1, y + h - 1);
        let _ = ctx.term.write("┘");

        for i in 1..w - 1 {
            let _ = ctx.term.move_to(x + i, y);
            let _ = ctx.term.write("─");
            let _ = ctx.term.move_to(x + i, y + h - 1);
            let _ = ctx.term.write("─");
        }

        for i in 1..h - 1 {
            let _ = ctx.term.move_to(x, y + i);
            let _ = ctx.term.write("│");
            let _ = ctx.term.move_to(x + w - 1, y + i);
            let _ = ctx.term.write("│");
        }
    }
}
