//! # pi-tui
//!
//! Terminal UI library with differential rendering, a component model, and
//! full keyboard protocol support (including Kitty keyboard protocol).
//!
//! ## Architecture
//!
//! - [`terminal`] — Abstract terminal trait + real (ProcessTerminal) and
//!   virtual (VirtualTerminal for testing) implementations.
//! - [`components`] — Component trait and all built-in components:
//!   Input, Editor, Markdown, SelectList, Container, Loader, Text.
//! - [`rendering`] — Differential renderer and synchronized output helpers.
//! - [`overlay`] — Overlay/popup management system.
//! - [`keyboard`] — Kitty keyboard protocol parser and configurable keybindings.

pub mod components;
pub mod keyboard;
pub mod overlay;
pub mod rendering;
pub mod terminal;

// Convenience re-exports for common types
pub use components::{
    Component, Container, Editor, Focusable, Input, InputResult, Loader, Markdown, SelectItem,
    SelectList, Spacer, Text, TruncatedText, TuiBox, CURSOR_MARKER,
};

pub use terminal::process::ProcessTerminal;
pub use terminal::virtual_term::VirtualTerminal;
pub use terminal::Terminal;

pub use rendering::synchronized::{begin_sync, end_sync};
pub use rendering::DifferentialRenderer;

pub use overlay::{OverlayAnchor, OverlayHandle, OverlayManager, OverlayOptions, SizeValue};

pub use keyboard::{EditorAction, KeybindingsManager};

pub use keyboard::kitty::{matches_key, parse_input, Key, KeyEvent, KeyEventType, Modifiers};
