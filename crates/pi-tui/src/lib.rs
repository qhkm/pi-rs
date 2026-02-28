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

pub mod terminal;
pub mod components;
pub mod rendering;
pub mod overlay;
pub mod keyboard;

// Convenience re-exports for common types
pub use components::{
    Component,
    InputResult,
    Focusable,
    CURSOR_MARKER,
    Editor,
    Input,
    Loader,
    Markdown,
    SelectList,
    SelectItem,
    Container,
    TuiBox,
    Spacer,
    Text,
    TruncatedText,
};

pub use terminal::Terminal;
pub use terminal::process::ProcessTerminal;
pub use terminal::virtual_term::VirtualTerminal;

pub use rendering::DifferentialRenderer;
pub use rendering::synchronized::{begin_sync, end_sync};

pub use overlay::{
    OverlayManager,
    OverlayOptions,
    OverlayAnchor,
    OverlayHandle,
    SizeValue,
};

pub use keyboard::{
    EditorAction,
    KeybindingsManager,
};

pub use keyboard::kitty::{
    Key,
    KeyEvent,
    KeyEventType,
    Modifiers,
    parse_input,
    matches_key,
};
