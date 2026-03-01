//! High-level TUI application framework.
//!
//! This module provides opinionated building blocks for creating interactive
//! terminal applications with minimal boilerplate.
//!
//! # Example
//!
//! ```rust,no_run
//! use pi_tui::app::{App, AppContext, AppResult};
//! use crossterm::event::KeyEvent;
//!
//! struct MyApp {
//!     counter: i32,
//! }
//!
//! fn handle_key(state: &mut MyApp, key: KeyEvent, ctx: &mut AppContext) -> AppResult {
//!     // Handle keyboard input
//!     AppResult::Continue
//! }
//!
//! fn render(state: &MyApp, ctx: &mut AppContext) {
//!     // Render the UI
//! }
//!
//! fn main() -> std::io::Result<()> {
//!     let app = App::new(MyApp { counter: 0 }, handle_key, render);
//!     let exit_code = app.run()?;
//!     std::process::exit(exit_code);
//! }
//! ```

pub mod framework;

pub use framework::{
    render::{box_border, hline, status_bar},
    App, AppContext, AppResult, FocusArea, LayoutApp,
};
