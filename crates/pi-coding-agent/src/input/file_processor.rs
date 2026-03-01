use std::path::{Path, PathBuf};

use anyhow::Result;

/// Result of processing user input for @file references.
#[derive(Debug)]
pub struct ProcessedInput {
    /// The text message (with file contents prepended via <file> tags).
    pub text: String,
    /// Image contents extracted from @image references.
    pub images: Vec<ImageAttachment>,
}

/// A raw image read from a referenced file.
#[derive(Debug)]
pub struct ImageAttachment {
    pub data: Vec<u8>,
    pub mime_type: String,
    pub filename: String,
}

const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "gif", "webp"];

/// Process user input, expanding `@file` references.
///
/// - `@path/to/file.txt` is read and wrapped in `<file name="...">...</file>` tags,
///   then prepended to the returned text.
/// - `@"path with spaces/file.txt"` supports quoted paths with spaces.
/// - `@path/to/image.png` is read as raw bytes and appended to the `images` list.
/// - Paths are resolved relative to `cwd`.
/// - If a referenced file does not exist, the `@reference` is kept as literal text.
///
/// Returns the processed input with expanded text and extracted images.
pub fn process_input(input: &str, cwd: &Path) -> Result<ProcessedInput> {
    let mut text_parts: Vec<String> = Vec::new();
    let mut images: Vec<ImageAttachment> = Vec::new();
    let mut remaining_text_parts: Vec<String> = Vec::new();

    // First, process quoted references @"..."
    let mut processed = input.to_string();
    let mut found_quoted = true;
    
    while found_quoted {
        found_quoted = false;
        if let Some(start) = processed.find('@') {
            if processed.chars().nth(start + 1) == Some('"') {
                if let Some(end) = processed[start + 2..].find('"') {
                    let end = start + 2 + end;
                    let quoted_ref = &processed[start + 2..end];
                    let before = &processed[..start];
                    let after = &processed[end + 1..];
                    
                    if let Ok(()) = process_file_ref(quoted_ref, cwd, &mut text_parts, &mut images) {
                        // File processed successfully
                        if !before.trim().is_empty() {
                            remaining_text_parts.push(before.trim().to_string());
                        }
                        if !after.trim().is_empty() {
                            remaining_text_parts.push(after.trim().to_string());
                        }
                        processed = String::new();
                        found_quoted = true;
                        break;
                    } else {
                        // File doesn't exist - keep as literal, mark as processed
                        processed = format!("{}@\"{}\"{}", before, quoted_ref, after);
                        // Skip this reference for the next loop
                        processed = processed.replacen("@\"", "@@\"", 1);
                    }
                }
            }
        }
    }
    
    // Restore @@ to @ for unmatched references
    processed = processed.replace("@@", "@");
    
    // Process remaining unquoted references
    let mut final_remaining = String::new();
    
    for word in processed.split_whitespace() {
        if word.starts_with('@') && word.len() > 1 {
            let file_ref = &word[1..];
            
            if let Err(_) = process_file_ref(file_ref, cwd, &mut text_parts, &mut images) {
                // File doesn't exist - keep as literal
                if !final_remaining.is_empty() {
                    final_remaining.push(' ');
                }
                final_remaining.push_str(word);
            }
        } else {
            if !final_remaining.is_empty() {
                final_remaining.push(' ');
            }
            final_remaining.push_str(word);
        }
    }
    
    if !final_remaining.is_empty() {
        remaining_text_parts.push(final_remaining);
    }

    // Combine: file contents first, then remaining user text.
    let remaining_text = remaining_text_parts.join(" ");
    if !remaining_text.is_empty() {
        text_parts.push(remaining_text);
    }

    Ok(ProcessedInput {
        text: text_parts.join("\n\n"),
        images,
    })
}

/// Process a single file reference (quoted or unquoted).
fn process_file_ref(
    file_ref: &str,
    cwd: &Path,
    text_parts: &mut Vec<String>,
    images: &mut Vec<ImageAttachment>,
) -> anyhow::Result<()> {
    let path = resolve_file_path(file_ref, cwd);

    if !path.exists() {
        anyhow::bail!("File not found: {}", file_ref);
    }

    if is_image_file(&path) {
        match std::fs::read(&path) {
            Ok(data) => {
                let mime = mime_type_for_path(&path);
                images.push(ImageAttachment {
                    data,
                    mime_type: mime,
                    filename: file_ref.to_string(),
                });
            }
            Err(e) => anyhow::bail!("Error reading image: {}", e),
        }
    } else {
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                text_parts.push(format!(
                    "<file name=\"{}\">\n{}\n</file>",
                    file_ref, content
                ));
            }
            Err(e) => anyhow::bail!("Error reading file: {}", e),
        }
    }
    
    Ok(())
}

impl ImageAttachment {
    /// Convert to a `pi_ai::Content::Image` block (base64-encoded).
    pub fn to_content(&self) -> pi_ai::Content {
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&self.data);
        pi_ai::Content::Image {
            data: b64,
            mime_type: self.mime_type.clone(),
        }
    }
}

fn resolve_file_path(file_ref: &str, cwd: &Path) -> PathBuf {
    let path = Path::new(file_ref);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

fn is_image_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| IMAGE_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
        .unwrap_or(false)
}

fn mime_type_for_path(path: &Path) -> String {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .as_deref()
    {
        Some("jpg" | "jpeg") => "image/jpeg".to_string(),
        Some("png") => "image/png".to_string(),
        Some("gif") => "image/gif".to_string(),
        Some("webp") => "image/webp".to_string(),
        _ => "application/octet-stream".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_no_at_references() {
        let tmp = TempDir::new().unwrap();
        let result = process_input("hello world", tmp.path()).unwrap();
        assert_eq!(result.text, "hello world");
        assert!(result.images.is_empty());
    }

    #[test]
    fn test_text_file_expansion() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("test.txt"), "file content here").unwrap();
        let result = process_input("check @test.txt please", tmp.path()).unwrap();
        assert!(result.text.contains("<file name=\"test.txt\">"));
        assert!(result.text.contains("file content here"));
        assert!(result.text.contains("check"));
        assert!(result.text.contains("please"));
    }

    #[test]
    fn test_image_file_extraction() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("photo.png"), b"fake png data").unwrap();
        let result = process_input("look at @photo.png", tmp.path()).unwrap();
        assert_eq!(result.images.len(), 1);
        assert_eq!(result.images[0].mime_type, "image/png");
        assert_eq!(result.images[0].data, b"fake png data");
    }

    #[test]
    fn test_nonexistent_file_kept_as_text() {
        let tmp = TempDir::new().unwrap();
        let result = process_input("@nonexistent.txt", tmp.path()).unwrap();
        assert!(result.text.contains("@nonexistent.txt"));
        assert!(result.images.is_empty());
    }

    #[test]
    fn test_multiple_files() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("a.txt"), "content a").unwrap();
        fs::write(tmp.path().join("b.txt"), "content b").unwrap();
        let result = process_input("@a.txt and @b.txt", tmp.path()).unwrap();
        assert!(result.text.contains("content a"));
        assert!(result.text.contains("content b"));
    }

    #[test]
    fn test_is_image_file() {
        assert!(is_image_file(Path::new("photo.png")));
        assert!(is_image_file(Path::new("photo.jpg")));
        assert!(is_image_file(Path::new("photo.JPEG")));
        assert!(is_image_file(Path::new("photo.webp")));
        assert!(!is_image_file(Path::new("file.txt")));
        assert!(!is_image_file(Path::new("file.rs")));
    }

    #[test]
    fn test_image_to_content_base64() {
        let attachment = ImageAttachment {
            data: b"test image data".to_vec(),
            mime_type: "image/png".to_string(),
            filename: "test.png".to_string(),
        };
        let content = attachment.to_content();
        match content {
            pi_ai::Content::Image { data, mime_type } => {
                use base64::Engine;
                let expected = base64::engine::general_purpose::STANDARD.encode(b"test image data");
                assert_eq!(data, expected);
                assert_eq!(mime_type, "image/png");
            }
            _ => panic!("Expected Image content variant"),
        }
    }

    #[test]
    fn test_mixed_text_and_images() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("code.rs"), "fn main() {}").unwrap();
        fs::write(tmp.path().join("screenshot.png"), b"png bytes").unwrap();
        let result =
            process_input("review @code.rs and look at @screenshot.png", tmp.path()).unwrap();
        assert!(result.text.contains("<file name=\"code.rs\">"));
        assert!(result.text.contains("fn main() {}"));
        assert!(result.text.contains("review"));
        assert!(result.text.contains("and look at"));
        assert_eq!(result.images.len(), 1);
        assert_eq!(result.images[0].filename, "screenshot.png");
    }

    #[test]
    fn test_absolute_path() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("abs.txt");
        fs::write(&file_path, "absolute content").unwrap();
        let input = format!("read @{}", file_path.display());
        let result = process_input(&input, tmp.path()).unwrap();
        assert!(result.text.contains("absolute content"));
    }

    #[test]
    fn test_empty_input() {
        let tmp = TempDir::new().unwrap();
        let result = process_input("", tmp.path()).unwrap();
        assert_eq!(result.text, "");
        assert!(result.images.is_empty());
    }

    #[test]
    fn test_at_sign_alone() {
        let tmp = TempDir::new().unwrap();
        let result = process_input("@ something", tmp.path()).unwrap();
        assert_eq!(result.text, "@ something");
        assert!(result.images.is_empty());
    }

    #[test]
    fn test_quoted_path_with_spaces() {
        let tmp = TempDir::new().unwrap();
        // Create file with spaces in name
        fs::write(tmp.path().join("file with spaces.txt"), "content with spaces").unwrap();
        
        let result = process_input("check @\"file with spaces.txt\" please", tmp.path()).unwrap();
        assert!(result.text.contains("<file name=\"file with spaces.txt\">"));
        assert!(result.text.contains("content with spaces"));
        assert!(result.text.contains("check"));
        assert!(result.text.contains("please"));
    }

    #[test]
    fn test_quoted_nonexistent_file_kept_as_text() {
        let tmp = TempDir::new().unwrap();
        let result = process_input("check @\"no such file.txt\" please", tmp.path()).unwrap();
        assert!(result.text.contains("check"));
        assert!(result.text.contains("@\"no such file.txt\""));
        assert!(result.text.contains("please"));
    }

    #[test]
    fn test_quoted_image_path() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("my image.png"), b"image data").unwrap();
        
        let result = process_input("look at @\"my image.png\"", tmp.path()).unwrap();
        assert_eq!(result.images.len(), 1);
        assert_eq!(result.images[0].filename, "my image.png");
        assert_eq!(result.images[0].data, b"image data");
    }
}
