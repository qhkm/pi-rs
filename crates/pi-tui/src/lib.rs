//! # pi-tui
//!
//! Terminal UI library with differential rendering, a component model, and
//! full keyboard protocol support (including Kitty keyboard protocol).
//!
//! ## Architecture
//!
//! - [`app`] — High-level application framework for building TUI apps.
//! - [`terminal`] — Abstract terminal trait + real (ProcessTerminal) and
//!   virtual (VirtualTerminal for testing) implementations.
//! - [`components`] — Component trait and 15+ built-in components:
//!   Input, Editor, Markdown, SelectList, Container, Loader, Text, Footer,
//!   Autocomplete, Diff, StreamingMessage, ToolExecution, ModelSelector,
//!   ThinkingSelector, QuickActionSelector.
//! - [`fuzzy`] — Fuzzy string matching for autocomplete and search.
//! - [`theme`] — Theme system with hot-reload support.
//! - [`slash`] — Slash command parser for TUI interactions.
//! - [`rendering`] — Differential renderer and synchronized output helpers.
//! - [`overlay`] — Overlay/popup management system.
//! - [`keyboard`] — Kitty keyboard protocol parser and configurable keybindings.
//! - [`image`] — Terminal image rendering (iTerm2 and Kitty graphics protocols).

pub mod app;
pub mod components;
pub mod fuzzy;
pub mod image;
pub mod keyboard;
pub mod overlay;
pub mod rendering;
pub mod slash;
pub mod terminal;
pub mod theme;

// Convenience re-exports for common types
pub use components::{
    Autocomplete, AutocompleteTheme, Component, Container, Diff, DiffHunk, DiffLine, DiffLineKind,
    DiffTheme, DiffViewMode, Editor, Focusable, Footer, FooterTheme, Input, InputResult, Loader,
    Markdown, ModelInfo, ModelSelector, QuickActionSelector, SelectItem, SelectList, Spacer,
    StreamingMessage, StreamingMessageList, StreamingState, StreamingTheme, Text, ThinkingLevel,
    ThinkingSelector, ToolExecution, ToolExecutionTheme, ToolExecutionView, ToolSpinner, ToolState,
    TruncatedText, TuiBox, CURSOR_MARKER,
};

pub use terminal::process::ProcessTerminal;
pub use terminal::virtual_term::VirtualTerminal;
pub use terminal::Terminal;

pub use rendering::synchronized::{begin_sync, end_sync};
pub use rendering::DifferentialRenderer;

pub use overlay::{OverlayAnchor, OverlayHandle, OverlayManager, OverlayOptions, SizeValue};

pub use keyboard::{EditorAction, KeybindingsManager};

pub use keyboard::kitty::{matches_key, parse_input, Key, KeyEvent, KeyEventType, Modifiers};

pub use fuzzy::{fuzzy_filter, fuzzy_match, highlight_matches, FuzzyMatch, MatchOptions};

pub use theme::{
    dark_theme, high_contrast_theme, light_theme, Color, ComponentTheme, Style, Theme, ThemeError,
    ThemeManager,
};

pub use slash::{
    complete_command, CommandDef, CommandResult, SimpleCommandHandler, SlashCommand,
    SlashCommandHandler, SlashCommandRegistry,
};

// Image rendering protocols
pub use image::iterm2::{is_iterm2_terminal, Iterm2Renderer};
pub use image::kitty::{is_kitty_terminal, KittyRenderer};
pub use image::{ImageProtocolRenderer, ImageRenderer, TerminalProtocol};

// App framework
pub use app::{App, AppContext, AppResult, FocusArea, LayoutApp};
