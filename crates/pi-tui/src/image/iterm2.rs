//! iTerm2 inline image protocol implementation
//!
//! Reference: <https://iterm2.com/documentation-images.html>
//!
//! The protocol uses OSC 1337 escape sequences with base64-encoded image data.
//! Format: `ESC ] 1337 ; File = [key=value;]* [base64 data] BEL`
//!
//! Supported terminals:
//! - iTerm2 (macOS)
//! - WezTerm (cross-platform)
//! - VS Code integrated terminal
//! - Hyper
//! - Tabby
//! - Rio (partial)

use std::fmt;
use std::io;
use std::path::Path;

/// Error types for iTerm2 image rendering
#[derive(Debug)]
pub enum Iterm2Error {
    /// IO error reading file
    Io(io::Error),
    /// Invalid image format
    InvalidImage,
    /// File too large
    FileTooLarge,
}

impl fmt::Display for Iterm2Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Iterm2Error::Io(e) => write!(f, "IO error: {e}"),
            Iterm2Error::InvalidImage => write!(f, "Invalid image format"),
            Iterm2Error::FileTooLarge => write!(f, "File too large for inline display"),
        }
    }
}

impl std::error::Error for Iterm2Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Iterm2Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for Iterm2Error {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// Maximum file size for inline images (10MB to avoid terminal performance issues)
const MAX_FILE_SIZE: usize = 10 * 1024 * 1024;

/// iTerm2 inline image renderer
#[derive(Debug, Clone, Copy, Default)]
pub struct Iterm2Renderer;

impl Iterm2Renderer {
    /// Check if the current terminal supports iTerm2 image protocol
    pub fn is_supported() -> bool {
        is_iterm2_terminal()
    }

    /// Render an image file as iTerm2 inline image escape sequence
    ///
    /// # Arguments
    /// * `path` - Path to the image file
    /// * `columns` - Optional display width in terminal columns
    /// * `rows` - Optional display height in terminal rows
    ///
    /// # Returns
    /// Escape sequence string to write to terminal
    pub fn render_file(
        path: impl AsRef<Path>,
        columns: Option<u32>,
        rows: Option<u32>,
    ) -> Result<String, Iterm2Error> {
        let data = std::fs::read(path)?;
        Self::render_data(&data, columns, rows)
    }

    /// Render image data as iTerm2 inline image escape sequence
    ///
    /// # Arguments
    /// * `data` - Raw image bytes
    /// * `columns` - Optional display width in terminal columns
    /// * `rows` - Optional display height in terminal rows
    ///
    /// # Returns
    /// Escape sequence string to write to terminal
    pub fn render_data(
        data: &[u8],
        columns: Option<u32>,
        rows: Option<u32>,
    ) -> Result<String, Iterm2Error> {
        if data.is_empty() {
            return Err(Iterm2Error::InvalidImage);
        }

        if data.len() > MAX_FILE_SIZE {
            return Err(Iterm2Error::FileTooLarge);
        }

        // Validate image format (check magic bytes)
        if !is_valid_image(data) {
            return Err(Iterm2Error::InvalidImage);
        }

        // Encode to base64
        let encoded = base64_encode(data);

        // Build the escape sequence
        let mut sequence = String::with_capacity(encoded.len() + 100);

        // OSC 1337 start: ESC ] 1337 ; File =
        sequence.push_str("\x1b]1337;File=");

        // Add parameters
        let mut params = Vec::new();

        // Always inline (not a download)
        params.push("inline=1".to_string());

        // Add dimensions if specified
        if let Some(cols) = columns {
            params.push(format!("width={cols}"));
        }
        if let Some(r) = rows {
            params.push(format!("height={r}"));
        }

        // Add size parameter for file size hint (helps terminal pre-allocate)
        params.push(format!("size={}", data.len()));

        sequence.push_str(&params.join(";"));
        sequence.push(':');

        // Add base64 encoded image data
        sequence.push_str(&encoded);

        // BEL terminator
        sequence.push('\x07');

        Ok(sequence)
    }

    /// Render an image at original size (preserving aspect ratio)
    pub fn render_original_size(data: &[u8]) -> Result<String, Iterm2Error> {
        Self::render_data(data, None, None)
    }

    /// Render an image with explicit pixel dimensions
    ///
    /// Note: iTerm2 supports 'width' and 'height' in pixels with 'px' suffix
    /// or as percentages with '%' suffix
    pub fn render_with_pixel_size(
        data: &[u8],
        width_px: Option<u32>,
        height_px: Option<u32>,
    ) -> Result<String, Iterm2Error> {
        if data.is_empty() {
            return Err(Iterm2Error::InvalidImage);
        }

        if data.len() > MAX_FILE_SIZE {
            return Err(Iterm2Error::FileTooLarge);
        }

        if !is_valid_image(data) {
            return Err(Iterm2Error::InvalidImage);
        }

        let encoded = base64_encode(data);
        let mut sequence = String::with_capacity(encoded.len() + 100);

        sequence.push_str("\x1b]1337;File=");

        let mut params = vec!["inline=1".to_string()];

        if let Some(w) = width_px {
            params.push(format!("width={w}px"));
        }
        if let Some(h) = height_px {
            params.push(format!("height={h}px"));
        }

        params.push(format!("size={}", data.len()));

        sequence.push_str(&params.join(";"));
        sequence.push(':');
        sequence.push_str(&encoded);
        sequence.push('\x07');

        Ok(sequence)
    }
}

/// Detect if the current terminal supports iTerm2 image protocol
pub fn is_iterm2_terminal() -> bool {
    // Check TERM_PROGRAM first (most reliable)
    if let Ok(term_program) = std::env::var("TERM_PROGRAM") {
        match term_program.as_str() {
            "iTerm.app" => return true,
            "WezTerm" => return true,
            _ => {}
        }
    }

    // Check LC_TERMINAL (iTerm2 sets this)
    if let Ok(lc_term) = std::env::var("LC_TERMINAL") {
        if lc_term == "iTerm2" {
            return true;
        }
    }

    // Check for VS Code terminal
    if std::env::var("TERM_PROGRAM").ok().as_deref() == Some("vscode") {
        return true;
    }

    // Check TERM for known compatible terminals
    if let Ok(term) = std::env::var("TERM") {
        // WezTerm sometimes doesn't set TERM_PROGRAM
        if term.contains("wezterm") {
            return true;
        }
        // Hyper terminal
        if term == "xterm-256color" && std::env::var("HYPER_VERSION").is_ok() {
            return true;
        }
    }

    false
}

/// Validate image format by checking magic bytes
fn is_valid_image(data: &[u8]) -> bool {
    if data.len() < 8 {
        return false;
    }

    // PNG: 89 50 4E 47 0D 0A 1A 0A
    if data.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
        return true;
    }

    // JPEG: FF D8 FF
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return true;
    }

    // GIF: GIF87a or GIF89a
    if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        return true;
    }

    // BMP: BM
    if data.starts_with(b"BM") {
        return true;
    }

    // WebP: RIFF....WEBP
    if data.starts_with(b"RIFF") && data.len() >= 12 && &data[8..12] == b"WEBP" {
        return true;
    }

    false
}

/// Base64 encode without padding (iTerm2 accepts both)
fn base64_encode(data: &[u8]) -> String {
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut result = String::with_capacity((data.len() + 2) / 3 * 4);

    for chunk in data.chunks(3) {
        let b = match chunk.len() {
            1 => [chunk[0], 0, 0],
            2 => [chunk[0], chunk[1], 0],
            3 => [chunk[0], chunk[1], chunk[2]],
            _ => unreachable!(),
        };

        let n = ((b[0] as usize) << 16) | ((b[1] as usize) << 8) | (b[2] as usize);

        result.push(CHARSET[(n >> 18) & 0x3F] as char);
        result.push(CHARSET[(n >> 12) & 0x3F] as char);

        if chunk.len() > 1 {
            result.push(CHARSET[(n >> 6) & 0x3F] as char);
        }

        if chunk.len() > 2 {
            result.push(CHARSET[n & 0x3F] as char);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_valid_image() {
        // PNG
        let png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00];
        assert!(is_valid_image(&png));

        // JPEG
        let jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46];
        assert!(is_valid_image(&jpeg));

        // GIF87a
        let gif87 = vec![b'G', b'I', b'F', b'8', b'7', b'a', 0x00, 0x00];
        assert!(is_valid_image(&gif87));

        // GIF89a
        let gif89 = vec![b'G', b'I', b'F', b'8', b'9', b'a', 0x00, 0x00];
        assert!(is_valid_image(&gif89));

        // BMP
        let bmp = vec![b'B', b'M', 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        assert!(is_valid_image(&bmp));

        // WebP
        let webp = vec![b'R', b'I', b'F', b'F', 0x00, 0x00, 0x00, 0x00, b'W', b'E', b'B', b'P'];
        assert!(is_valid_image(&webp));

        // Invalid
        assert!(!is_valid_image(b"not an image"));
        assert!(!is_valid_image(b""));
        assert!(!is_valid_image(b"RIFF")); // Too short for WebP
    }

    #[test]
    fn test_base64_encode() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg");
        assert_eq!(base64_encode(b"fo"), "Zm8");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn test_render_data_empty() {
        let result = Iterm2Renderer::render_data(b"", None, None);
        assert!(matches!(result, Err(Iterm2Error::InvalidImage)));
    }

    #[test]
    fn test_render_data_invalid() {
        let result = Iterm2Renderer::render_data(b"not an image", None, None);
        assert!(matches!(result, Err(Iterm2Error::InvalidImage)));
    }

    #[test]
    fn test_render_data_valid_png() {
        let png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00];
        let result = Iterm2Renderer::render_data(&png, Some(80), Some(24));
        assert!(result.is_ok());

        let seq = result.unwrap();
        assert!(seq.starts_with("\x1b]1337;File="));
        assert!(seq.contains("inline=1"));
        assert!(seq.contains("inline=1;"));
        assert!(seq.contains("width=80"));
        assert!(seq.contains("height=24"));
        assert!(seq.ends_with('\x07'));
    }

    #[test]
    fn test_render_data_no_dimensions() {
        let png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00];
        let result = Iterm2Renderer::render_data(&png, None, None);
        assert!(result.is_ok());

        let seq = result.unwrap();
        assert!(seq.contains("inline=1"));
        assert!(!seq.contains("width="));
        assert!(!seq.contains("height="));
    }

    #[test]
    fn test_render_with_pixel_size() {
        let png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00];
        let result = Iterm2Renderer::render_with_pixel_size(&png, Some(100), Some(200));
        assert!(result.is_ok());

        let seq = result.unwrap();
        assert!(seq.contains("width=100px"));
        assert!(seq.contains("height=200px"));
    }

    #[test]
    fn test_file_too_large() {
        let large_data = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]
            .into_iter()
            .chain(std::iter::repeat(0).take(MAX_FILE_SIZE + 1))
            .collect::<Vec<_>>();

        let result = Iterm2Renderer::render_data(&large_data, None, None);
        assert!(matches!(result, Err(Iterm2Error::FileTooLarge)));
    }
}
