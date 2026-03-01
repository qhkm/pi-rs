//! Terminal image rendering for iTerm2 and Kitty protocols.
//!
//! This module provides support for displaying inline images in terminals
//! that support the iTerm2 or Kitty image protocols.

use std::fmt;

/// Supported terminal image protocols.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageProtocol {
    /// iTerm2 inline image protocol (OSC 1337)
    Iterm2,
    /// Kitty graphics protocol (APC)
    Kitty,
}

/// Detect which image protocol is supported by the current terminal.
///
/// Returns `None` if no supported protocol is detected.
pub fn detect_protocol() -> Option<ImageProtocol> {
    // Check for Kitty first (more specific)
    if std::env::var("KITTY_WINDOW_ID").is_ok() {
        return Some(ImageProtocol::Kitty);
    }
    
    // Check for iTerm2
    if let Ok(term_program) = std::env::var("TERM_PROGRAM") {
        if term_program == "iTerm.app" {
            return Some(ImageProtocol::Iterm2);
        }
    }
    
    // Check TERM for kitty
    if let Ok(term) = std::env::var("TERM") {
        if term.contains("kitty") {
            return Some(ImageProtocol::Kitty);
        }
    }
    
    None
}

/// Configuration for image rendering.
#[derive(Debug, Clone)]
pub struct ImageConfig {
    /// Maximum width in pixels (0 = auto)
    pub max_width: u32,
    /// Maximum height in pixels (0 = auto)
    pub max_height: u32,
    /// Whether to preserve aspect ratio
    pub preserve_aspect: bool,
}

impl Default for ImageConfig {
    fn default() -> Self {
        Self {
            max_width: 0,
            max_height: 0,
            preserve_aspect: true,
        }
    }
}

/// An image ready to be rendered in the terminal.
pub struct TerminalImage {
    /// Raw image data
    data: Vec<u8>,
    /// MIME type (e.g., "image/png")
    mime_type: String,
    /// File name
    filename: String,
    /// Configuration
    config: ImageConfig,
}

impl TerminalImage {
    /// Create a new terminal image from raw data.
    pub fn new(
        data: Vec<u8>,
        mime_type: impl Into<String>,
        filename: impl Into<String>,
    ) -> Self {
        Self {
            data,
            mime_type: mime_type.into(),
            filename: filename.into(),
            config: ImageConfig::default(),
        }
    }
    
    /// Set configuration.
    pub fn with_config(mut self, config: ImageConfig) -> Self {
        self.config = config;
        self
    }
    
    /// Render the image for the given protocol.
    ///
    /// Returns a string containing the escape sequences to display the image.
    pub fn render(&self, protocol: ImageProtocol) -> String {
        match protocol {
            ImageProtocol::Iterm2 => self.render_iterm2(),
            ImageProtocol::Kitty => self.render_kitty(),
        }
    }
    
    /// Render using iTerm2 protocol (OSC 1337).
    ///
    /// Format: ESC]1337;File=[arguments]:[base64 data]BEL
    fn render_iterm2(&self) -> String {
        use base64::Engine;
        
        let b64 = base64::engine::general_purpose::STANDARD.encode(&self.data);
        
        let mut args = format!("name={}", base64::engine::general_purpose::STANDARD.encode(&self.filename));
        args.push_str(&format!(";size={}", self.data.len()));
        
        if self.config.max_width > 0 {
            args.push_str(&format!(";width={}", self.config.max_width));
        }
        if self.config.max_height > 0 {
            args.push_str(&format!(";height={}", self.config.max_height));
        }
        if self.config.preserve_aspect {
            args.push_str(";preserveAspectRatio=1");
        } else {
            args.push_str(";preserveAspectRatio=0");
        }
        
        format!("\x1b]1337;File={}:{:?}\x07", args, b64)
    }
    
    /// Render using Kitty graphics protocol (APC).
    ///
    /// Format: APC G [key=value,...] ; [base64 data] ST
    fn render_kitty(&self) -> String {
        use base64::Engine;
        
        let b64 = base64::engine::general_purpose::STANDARD.encode(&self.data);
        
        // Kitty protocol uses chunking for large images (4096 bytes max per chunk)
        let chunk_size = 4096;
        let mut result = String::new();
        
        if b64.len() <= chunk_size {
            // Single chunk
            let action = "a=T"; // Transmit and display
            let format = match self.mime_type.as_str() {
                "image/png" => "f=100",
                "image/jpeg" | "image/jpg" => "f=101",
                "image/gif" => "f=102",
                "image/webp" => "f=103",
                _ => "f=100", // Default to PNG
            };
            let width = if self.config.max_width > 0 {
                format!(",c={}", self.config.max_width)
            } else {
                String::new()
            };
            let height = if self.config.max_height > 0 {
                format!(",r={}", self.config.max_height)
            } else {
                String::new()
            };
            
            result.push_str(&format!(
                "\x1b_G{},{}{}{};{}\x1b\\",
                action, format, width, height, b64
            ));
        } else {
            // Multi-chunk transmission
            let chunks: Vec<&str> = b64.as_bytes()
                .chunks(chunk_size)
                .map(|c| std::str::from_utf8(c).unwrap())
                .collect();
            
            for (i, chunk) in chunks.iter().enumerate() {
                let more = if i < chunks.len() - 1 { "m=1" } else { "m=0" };
                
                if i == 0 {
                    // First chunk
                    let format = match self.mime_type.as_str() {
                        "image/png" => "f=100",
                        "image/jpeg" | "image/jpg" => "f=101",
                        "image/gif" => "f=102",
                        "image/webp" => "f=103",
                        _ => "f=100",
                    };
                    let width = if self.config.max_width > 0 {
                        format!(",c={}", self.config.max_width)
                    } else {
                        String::new()
                    };
                    let height = if self.config.max_height > 0 {
                        format!(",r={}", self.config.max_height)
                    } else {
                        String::new()
                    };
                    result.push_str(&format!(
                        "\x1b_Ga=T,{}{}{}{};{}\x1b\\",
                        format, width, height, more, chunk
                    ));
                } else {
                    // Continuation chunk
                    result.push_str(&format!("\x1b_G{};{}\x1b\\", more, chunk));
                }
            }
        }
        
        result
    }
    
    /// Delete the image from the terminal (Kitty only).
    ///
    /// For iTerm2, images cannot be deleted once displayed.
    pub fn delete_kitty(&self, image_id: u32) -> String {
        format!("\x1b_Ga=d,d=I,i={}\x1b\\", image_id)
    }
}

impl fmt::Debug for TerminalImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TerminalImage")
            .field("size", &self.data.len())
            .field("mime_type", &self.mime_type)
            .field("filename", &self.filename)
            .finish()
    }
}

/// Render an image file to the terminal if a supported protocol is available.
///
/// Returns `Ok(Some(rendered))` if the image was rendered, `Ok(None)` if no
/// supported protocol is available, or `Err` if there was an error reading the file.
pub fn render_image_file(
    path: &std::path::Path,
    config: Option<ImageConfig>,
) -> Result<Option<String>, std::io::Error> {
    let protocol = match detect_protocol() {
        Some(p) => p,
        None => return Ok(None),
    };
    
    let data = std::fs::read(path)?;
    let filename = path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("image")
        .to_string();
    
    let mime_type = match path.extension().and_then(|e| e.to_str()) {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => "application/octet-stream",
    };
    
    let image = TerminalImage::new(data, mime_type, filename);
    let image = if let Some(cfg) = config {
        image.with_config(cfg)
    } else {
        image
    };
    
    Ok(Some(image.render(protocol)))
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_detect_kitty_via_env() {
        std::env::set_var("KITTY_WINDOW_ID", "1");
        assert_eq!(detect_protocol(), Some(ImageProtocol::Kitty));
        std::env::remove_var("KITTY_WINDOW_ID");
    }
    
    #[test]
    fn test_detect_iterm2_via_term_program() {
        std::env::set_var("TERM_PROGRAM", "iTerm.app");
        assert_eq!(detect_protocol(), Some(ImageProtocol::Iterm2));
        std::env::remove_var("TERM_PROGRAM");
    }
    
    #[test]
    fn test_no_protocol_detected() {
        std::env::remove_var("KITTY_WINDOW_ID");
        std::env::remove_var("TERM_PROGRAM");
        std::env::remove_var("TERM");
        assert_eq!(detect_protocol(), None);
    }
    
    #[test]
    fn test_terminal_image_creation() {
        let _image = TerminalImage::new(
            vec![1, 2, 3, 4],
            "image/png",
            "test.png"
        );
        // Test passes if we can create the image
    }
    
    #[test]
    fn test_image_config_default() {
        let config = ImageConfig::default();
        assert_eq!(config.max_width, 0);
        assert_eq!(config.max_height, 0);
        assert!(config.preserve_aspect);
    }
    
    #[test]
    fn test_kitty_delete_sequence() {
        let image = TerminalImage::new(vec![], "image/png", "test.png");
        let delete = image.delete_kitty(42);
        assert!(delete.contains("a=d"));
        assert!(delete.contains("i=42"));
    }
}
