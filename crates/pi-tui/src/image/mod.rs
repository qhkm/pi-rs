//! Terminal image rendering protocols
//!
//! Supports two inline image protocols:
//! - **iTerm2** (OSC 1337) — macOS/iTerm2, WezTerm, VS Code integrated terminal
//! - **Kitty** (APC) — Kitty terminal with shared memory or direct transmission
//!
//! # Example
//! ```
//! use pi_tui::image::{ImageRenderer, TerminalProtocol};
//!
//! let renderer = ImageRenderer::detect();
//! if let Some(img) = renderer.render_file("/path/to/image.png", Some(80), Some(24)) {
//!     println!("{}", img);
//! }
//! ```

pub mod iterm2;
pub mod kitty;

pub use iterm2::Iterm2Renderer;
pub use kitty::KittyRenderer;

use std::fmt;

/// Detected terminal graphics protocol
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalProtocol {
    /// iTerm2 inline image protocol (OSC 1337)
    Iterm2,
    /// Kitty graphics protocol (APC sequences)
    Kitty,
    /// No known graphics protocol supported
    None,
}

impl TerminalProtocol {
    /// Detect the best available protocol for the current terminal
    pub fn detect() -> Self {
        // Check for Kitty first (more specific)
        if kitty::is_kitty_terminal() {
            return Self::Kitty;
        }

        // Check for iTerm2-compatible terminals
        if iterm2::is_iterm2_terminal() {
            return Self::Iterm2;
        }

        Self::None
    }
}

impl fmt::Display for TerminalProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TerminalProtocol::Iterm2 => write!(f, "iTerm2"),
            TerminalProtocol::Kitty => write!(f, "Kitty"),
            TerminalProtocol::None => write!(f, "None"),
        }
    }
}

/// Generic image renderer that uses the best available protocol
#[derive(Debug, Clone)]
pub struct ImageRenderer {
    protocol: TerminalProtocol,
}

impl ImageRenderer {
    /// Create a new renderer with auto-detected protocol
    pub fn detect() -> Self {
        Self {
            protocol: TerminalProtocol::detect(),
        }
    }

    /// Create a new renderer with a specific protocol
    pub fn with_protocol(protocol: TerminalProtocol) -> Self {
        Self { protocol }
    }

    /// Get the protocol being used
    pub fn protocol(&self) -> TerminalProtocol {
        self.protocol
    }

    /// Check if any image protocol is available
    pub fn is_supported(&self) -> bool {
        self.protocol != TerminalProtocol::None
    }

    /// Render an image file inline
    ///
    /// # Arguments
    /// * `path` - Path to the image file
    /// * `columns` - Optional display width in columns (None for auto)
    /// * `rows` - Optional display height in rows (None for auto)
    ///
    /// # Returns
    /// Escape sequence string to write to terminal, or None if unsupported
    pub fn render_file(
        &self,
        path: impl AsRef<std::path::Path>,
        columns: Option<u32>,
        rows: Option<u32>,
    ) -> Option<String> {
        match self.protocol {
            TerminalProtocol::Iterm2 => Iterm2Renderer::render_file(path, columns, rows).ok(),
            TerminalProtocol::Kitty => KittyRenderer::render_file(path, columns, rows).ok(),
            TerminalProtocol::None => None,
        }
    }

    /// Render image data inline
    ///
    /// # Arguments
    /// * `data` - Raw image bytes
    /// * `columns` - Optional display width in columns (None for auto)
    /// * `rows` - Optional display height in rows (None for auto)
    ///
    /// # Returns
    /// Escape sequence string to write to terminal, or None if unsupported
    pub fn render_data(
        &self,
        data: &[u8],
        columns: Option<u32>,
        rows: Option<u32>,
    ) -> Option<String> {
        match self.protocol {
            TerminalProtocol::Iterm2 => Iterm2Renderer::render_data(data, columns, rows).ok(),
            TerminalProtocol::Kitty => KittyRenderer::render_data(data, columns, rows).ok(),
            TerminalProtocol::None => None,
        }
    }

    /// Generate escape sequence to delete all inline images
    ///
    /// This is useful when clearing the screen or handling cleanup
    pub fn clear_images(&self) -> Option<String> {
        match self.protocol {
            TerminalProtocol::Iterm2 => None, // iTerm2 doesn't have a clear command
            TerminalProtocol::Kitty => Some(KittyRenderer::clear_all_images()),
            TerminalProtocol::None => None,
        }
    }
}

impl Default for ImageRenderer {
    fn default() -> Self {
        Self::detect()
    }
}

/// Trait for protocol-specific renderers
pub trait ImageProtocolRenderer {
    /// Error type for rendering operations
    type Error: std::error::Error;

    /// Render an image file
    fn render_file(
        path: impl AsRef<std::path::Path>,
        columns: Option<u32>,
        rows: Option<u32>,
    ) -> Result<String, Self::Error>;

    /// Render image data
    fn render_data(
        data: &[u8],
        columns: Option<u32>,
        rows: Option<u32>,
    ) -> Result<String, Self::Error>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_protocol_display() {
        assert_eq!(TerminalProtocol::Iterm2.to_string(), "iTerm2");
        assert_eq!(TerminalProtocol::Kitty.to_string(), "Kitty");
        assert_eq!(TerminalProtocol::None.to_string(), "None");
    }

    #[test]
    fn test_renderer_default() {
        let renderer = ImageRenderer::default();
        // Should detect based on environment
        let _ = renderer.protocol();
    }

    #[test]
    fn test_renderer_with_protocol() {
        let renderer = ImageRenderer::with_protocol(TerminalProtocol::Iterm2);
        assert_eq!(renderer.protocol(), TerminalProtocol::Iterm2);
        assert!(renderer.is_supported());

        let renderer = ImageRenderer::with_protocol(TerminalProtocol::None);
        assert!(!renderer.is_supported());
    }
}
