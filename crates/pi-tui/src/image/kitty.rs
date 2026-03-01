//! Kitty graphics protocol implementation
//!
//! Reference: <https://sw.kovidgoyal.net/kitty/graphics-protocol/>
//!
//! The protocol uses APC (Application Program Command) escape sequences.
//! Format: `ESC _ G [key=value,...] [payload] ESC \`
//!
//! Supports:
//! - Direct transmission (entire image in one escape sequence)
//! - Chunked transmission (for large images)
//! - Shared memory transmission (faster, Unix only)
//! - Image placement and deletion

use std::fmt;
use std::io;
use std::path::Path;

/// Error types for Kitty graphics rendering
#[derive(Debug)]
pub enum KittyError {
    /// IO error reading file
    Io(io::Error),
    /// Invalid image format
    InvalidImage,
    /// File too large
    FileTooLarge,
    /// Protocol error
    Protocol(String),
}

impl fmt::Display for KittyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KittyError::Io(e) => write!(f, "IO error: {e}"),
            KittyError::InvalidImage => write!(f, "Invalid image format"),
            KittyError::FileTooLarge => write!(f, "File too large for inline display"),
            KittyError::Protocol(msg) => write!(f, "Protocol error: {msg}"),
        }
    }
}

impl std::error::Error for KittyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            KittyError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for KittyError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// Maximum chunk size for base64 transmission (4KB to stay under typical buffer limits)
const MAX_CHUNK_SIZE: usize = 4096;

/// Maximum total file size (50MB for Kitty, which handles larger files better)
const MAX_FILE_SIZE: usize = 50 * 1024 * 1024;

/// Image transmission medium
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransmissionMedium {
    /// Direct (embedded in escape sequence)
    Direct,
    /// File path (terminal reads the file)
    File,
    /// Temporary file ( Kitty deletes after display)
    TemporaryFile,
    /// Shared memory (Unix only, fastest)
    #[cfg(unix)]
    SharedMemory,
}

impl TransmissionMedium {
    fn as_str(&self) -> &'static str {
        match self {
            TransmissionMedium::Direct => "d",
            TransmissionMedium::File => "f",
            TransmissionMedium::TemporaryFile => "t",
            #[cfg(unix)]
            TransmissionMedium::SharedMemory => "s",
        }
    }
}

/// Image format for Kitty protocol
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    /// Portable Network Graphics
    Png,
    /// JPEG
    Jpeg,
    /// Graphics Interchange Format
    Gif,
    /// WebP
    Webp,
    /// Raw RGB data (32-bit)
    Rgb,
    /// Raw RGBA data (32-bit with alpha)
    Rgba,
    /// Automatic detection from data
    Auto,
}

impl ImageFormat {
    fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 8 {
            return None;
        }

        // PNG
        if data.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
            return Some(ImageFormat::Png);
        }

        // JPEG
        if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
            return Some(ImageFormat::Jpeg);
        }

        // GIF
        if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
            return Some(ImageFormat::Gif);
        }

        // WebP
        if data.starts_with(b"RIFF") && data.len() >= 12 && &data[8..12] == b"WEBP" {
            return Some(ImageFormat::Webp);
        }

        None
    }

    fn as_str(&self) -> Option<&'static str> {
        match self {
            ImageFormat::Png => Some("24"),
            ImageFormat::Jpeg => Some("24"), // Kitty doesn't distinguish JPEG format code
            ImageFormat::Gif => Some("24"),
            ImageFormat::Webp => Some("24"),
            ImageFormat::Rgb => Some("24"),
            ImageFormat::Rgba => Some("32"),
            ImageFormat::Auto => None,
        }
    }
}

/// Kitty graphics protocol renderer
#[derive(Debug, Clone, Copy, Default)]
pub struct KittyRenderer;

impl KittyRenderer {
    /// Check if the current terminal supports Kitty graphics protocol
    pub fn is_supported() -> bool {
        is_kitty_terminal()
    }

    /// Render an image file using Kitty graphics protocol
    ///
    /// # Arguments
    /// * `path` - Path to the image file
    /// * `columns` - Optional display width in terminal columns
    /// * `rows` - Optional display height in terminal rows
    ///
    /// # Returns
    /// Escape sequence string(s) to write to terminal
    pub fn render_file(
        path: impl AsRef<Path>,
        columns: Option<u32>,
        rows: Option<u32>,
    ) -> Result<String, KittyError> {
        let data = std::fs::read(path)?;
        Self::render_data(&data, columns, rows)
    }

    /// Render image data using Kitty graphics protocol
    ///
    /// Uses chunked transmission for large images to avoid terminal buffer limits.
    ///
    /// # Arguments
    /// * `data` - Raw image bytes
    /// * `columns` - Optional display width in terminal columns
    /// * `rows` - Optional display height in terminal rows
    ///
    /// # Returns
    /// Escape sequence string(s) to write to terminal
    pub fn render_data(
        data: &[u8],
        columns: Option<u32>,
        rows: Option<u32>,
    ) -> Result<String, KittyError> {
        if data.is_empty() {
            return Err(KittyError::InvalidImage);
        }

        if data.len() > MAX_FILE_SIZE {
            return Err(KittyError::FileTooLarge);
        }

        // Detect format
        let format = ImageFormat::from_bytes(data).ok_or(KittyError::InvalidImage)?;

        // Encode to base64
        let encoded = base64_encode(data);

        // Generate a unique image ID
        let image_id = generate_image_id();

        // Build the transmission sequence
        let mut result = String::with_capacity(encoded.len() + 200);

        // Check if we need chunking
        if encoded.len() <= MAX_CHUNK_SIZE {
            // Single transmission
            Self::append_transmission(
                &mut result,
                image_id,
                format,
                TransmissionMedium::Direct,
                columns,
                rows,
                &encoded,
                true,
            );
        } else {
            // Chunked transmission
            let chunks: Vec<&str> = encoded
                .as_bytes()
                .chunks(MAX_CHUNK_SIZE)
                .map(|c| std::str::from_utf8(c).unwrap())
                .collect();

            for (i, chunk) in chunks.iter().enumerate() {
                let is_last = i == chunks.len() - 1;
                let medium = if i == 0 {
                    TransmissionMedium::Direct
                } else {
                    TransmissionMedium::Direct // Continue with same medium
                };

                Self::append_transmission(
                    &mut result,
                    image_id,
                    format,
                    medium,
                    if is_last { columns } else { None },
                    if is_last { rows } else { None },
                    chunk,
                    is_last,
                );
            }
        }

        Ok(result)
    }

    /// Generate escape sequence to display a previously transmitted image
    ///
    /// # Arguments
    /// * `image_id` - The ID of the transmitted image
    /// * `columns` - Display width in columns
    /// * `rows` - Display height in rows
    /// * `x` - X offset within cell (0-100)
    /// * `y` - Y offset within cell (0-100)
    pub fn display_image(
        image_id: u32,
        columns: u32,
        rows: u32,
        x: Option<u8>,
        y: Option<u8>,
    ) -> String {
        let mut seq = String::with_capacity(100);

        // APC start: ESC _ G
        seq.push_str("\x1b_G");

        // Display command (a=T means display)
        seq.push_str("a=T");
        seq.push_str(&format!(",i={image_id},c={columns},r={rows}"));

        if let Some(x_pos) = x {
            seq.push_str(&format!(",x={x_pos}"));
        }
        if let Some(y_pos) = y {
            seq.push_str(&format!(",y={y_pos}"));
        }

        // APC end: ESC \
        seq.push_str("\x1b\\");

        seq
    }

    /// Generate escape sequence to delete all images
    pub fn clear_all_images() -> String {
        // Delete all images (a=d with no ID)
        "\x1b_Ga=d,d=A\x1b\\".to_string()
    }

    /// Generate escape sequence to delete a specific image
    pub fn clear_image(image_id: u32) -> String {
        format!("\x1b_Ga=d,d=I,i={image_id}\x1b\\")
    }

    /// Generate escape sequence to delete images by z-index
    pub fn clear_by_zindex(z: i32) -> String {
        format!("\x1b_Ga=d,d=Z,z={z}\x1b\\")
    }

    /// Render image with explicit placement control
    ///
    /// # Arguments
    /// * `data` - Raw image bytes
    /// * `placement_id` - Unique ID for this placement (allows updating)
    /// * `columns` - Display width in columns
    /// * `rows` - Display height in rows
    /// * `z` - Z-index for layering (negative = below text, positive = above)
    pub fn render_with_placement(
        data: &[u8],
        placement_id: u32,
        columns: u32,
        rows: u32,
        z: Option<i32>,
    ) -> Result<String, KittyError> {
        if data.is_empty() {
            return Err(KittyError::InvalidImage);
        }

        let format = ImageFormat::from_bytes(data).ok_or(KittyError::InvalidImage)?;
        let encoded = base64_encode(data);
        let image_id = generate_image_id();

        let mut result = String::with_capacity(encoded.len() + 200);

        // Transmission with placement in one command
        result.push_str("\x1b_Ga=T"); // Transmit and display
        result.push_str(&format!(",i={image_id},p={placement_id}"));
        result.push_str(&format!(",c={columns},r={rows}"));

        if let Some(z_index) = z {
            result.push_str(&format!(",z={z_index}"));
        }

        if let Some(fmt) = format.as_str() {
            result.push_str(&format!(",f={fmt}"));
        }

        result.push_str(",m=1"); // More data follows (base64)
        result.push('=');
        result.push_str(&encoded);
        result.push_str("\x1b\\");

        Ok(result)
    }

    fn append_transmission(
        result: &mut String,
        image_id: u32,
        format: ImageFormat,
        medium: TransmissionMedium,
        columns: Option<u32>,
        rows: Option<u32>,
        data: &str,
        is_last: bool,
    ) {
        // APC start: ESC _ G
        result.push_str("\x1b_G");

        // Action: transmit (a=t)
        result.push_str("a=t");

        // Image ID
        result.push_str(&format!(",i={image_id}"));

        // Format
        if let Some(fmt) = format.as_str() {
            result.push_str(&format!(",f={fmt}"));
        }

        // Transmission medium
        result.push_str(&format!(",t={}", medium.as_str()));

        // Dimensions (only on last chunk or single transmission)
        if is_last {
            if let Some(c) = columns {
                result.push_str(&format!(",c={c}"));
            }
            if let Some(r) = rows {
                result.push_str(&format!(",r={r}"));
            }
        }

        // More chunks flag
        if !is_last {
            result.push_str(",m=1");
        }

        // Data
        result.push('=');
        result.push_str(data);

        // APC end: ESC \
        result.push_str("\x1b\\");
    }
}

/// Detect if the current terminal is Kitty
pub fn is_kitty_terminal() -> bool {
    // Check KITTY_WINDOW_ID (most reliable)
    if std::env::var("KITTY_WINDOW_ID").is_ok() {
        return true;
    }

    // Check TERM
    if let Ok(term) = std::env::var("TERM") {
        if term.contains("kitty") {
            return true;
        }
    }

    // Check TERMINFO
    if let Ok(terminfo) = std::env::var("TERMINFO") {
        if terminfo.contains("kitty") {
            return true;
        }
    }

    false
}

/// Generate a pseudo-unique image ID
fn generate_image_id() -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(1);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// Base64 encode data
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
        } else {
            result.push('=');
        }

        if chunk.len() > 2 {
            result.push(CHARSET[n & 0x3F] as char);
        } else {
            result.push('=');
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_image_format_from_bytes() {
        // PNG
        let png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00];
        assert_eq!(ImageFormat::from_bytes(&png), Some(ImageFormat::Png));

        // JPEG
        let jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46];
        assert_eq!(ImageFormat::from_bytes(&jpeg), Some(ImageFormat::Jpeg));

        // GIF
        let gif = vec![b'G', b'I', b'F', b'8', b'9', b'a', 0x00, 0x00];
        assert_eq!(ImageFormat::from_bytes(&gif), Some(ImageFormat::Gif));

        // WebP
        let webp = vec![b'R', b'I', b'F', b'F', 0x00, 0x00, 0x00, 0x00, b'W', b'E', b'B', b'P'];
        assert_eq!(ImageFormat::from_bytes(&webp), Some(ImageFormat::Webp));

        // Invalid
        assert_eq!(ImageFormat::from_bytes(b"not an image"), None);
        assert_eq!(ImageFormat::from_bytes(b""), None);
    }

    #[test]
    fn test_base64_encode() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn test_generate_image_id() {
        let id1 = generate_image_id();
        let id2 = generate_image_id();
        let id3 = generate_image_id();

        assert_ne!(id1, id2);
        assert_ne!(id2, id3);
        assert!(id1 > 0);
    }

    #[test]
    fn test_render_data_empty() {
        let result = KittyRenderer::render_data(b"", None, None);
        assert!(matches!(result, Err(KittyError::InvalidImage)));
    }

    #[test]
    fn test_render_data_invalid() {
        let result = KittyRenderer::render_data(b"not an image", None, None);
        assert!(matches!(result, Err(KittyError::InvalidImage)));
    }

    #[test]
    fn test_render_data_valid_png() {
        let png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00];
        let result = KittyRenderer::render_data(&png, Some(80), Some(24));
        assert!(result.is_ok());

        let seq = result.unwrap();
        assert!(seq.contains("\x1b_G")); // APC start
        assert!(seq.contains("a=t")); // Transmit action
        assert!(seq.contains("i=")); // Image ID
        assert!(seq.contains("c=80")); // Columns
        assert!(seq.contains("r=24")); // Rows
        assert!(seq.contains("\x1b\\")); // APC end
    }

    #[test]
    fn test_render_data_no_dimensions() {
        let png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00];
        let result = KittyRenderer::render_data(&png, None, None);
        assert!(result.is_ok());

        let seq = result.unwrap();
        assert!(!seq.contains("c="));
        assert!(!seq.contains("r="));
    }

    #[test]
    fn test_clear_commands() {
        let clear_all = KittyRenderer::clear_all_images();
        assert!(clear_all.contains("a=d"));
        assert!(clear_all.contains("d=A"));

        let clear_one = KittyRenderer::clear_image(42);
        assert!(clear_one.contains("a=d"));
        assert!(clear_one.contains("d=I"));
        assert!(clear_one.contains("i=42"));

        let clear_z = KittyRenderer::clear_by_zindex(5);
        assert!(clear_z.contains("a=d"));
        assert!(clear_z.contains("d=Z"));
        assert!(clear_z.contains("z=5"));
    }

    #[test]
    fn test_display_image() {
        let seq = KittyRenderer::display_image(123, 80, 24, None, None);
        assert!(seq.contains("a=T")); // Display action
        assert!(seq.contains("i=123"));
        assert!(seq.contains("c=80"));
        assert!(seq.contains("r=24"));
    }

    #[test]
    fn test_display_with_offsets() {
        let seq = KittyRenderer::display_image(123, 80, 24, Some(50), Some(25));
        assert!(seq.contains("x=50"));
        assert!(seq.contains("y=25"));
    }

    #[test]
    fn test_render_with_placement() {
        let png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00];
        let result = KittyRenderer::render_with_placement(&png, 1, 80, 24, Some(-1));
        assert!(result.is_ok());

        let seq = result.unwrap();
        assert!(seq.contains("a=T"));
        assert!(seq.contains("p=1"));
        assert!(seq.contains("z=-1"));
    }

    #[test]
    fn test_transmission_medium_as_str() {
        assert_eq!(TransmissionMedium::Direct.as_str(), "d");
        assert_eq!(TransmissionMedium::File.as_str(), "f");
        assert_eq!(TransmissionMedium::TemporaryFile.as_str(), "t");
    }

    #[test]
    fn test_file_too_large() {
        let large_data = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]
            .into_iter()
            .chain(std::iter::repeat(0).take(MAX_FILE_SIZE + 1))
            .collect::<Vec<_>>();

        let result = KittyRenderer::render_data(&large_data, None, None);
        assert!(matches!(result, Err(KittyError::FileTooLarge)));
    }
}
