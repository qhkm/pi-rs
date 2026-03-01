use std::path::{Path, PathBuf};

use anyhow::Result;
use glob::glob as glob_match;

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

/// Maximum number of files that can be expanded from a single directory or glob reference.
const MAX_EXPANSION_FILES: usize = 100;

/// Process user input, expanding `@file` references.
///
/// - `@path/to/file.txt` is read and wrapped in `<file name="...">...</file>` tags,
///   then prepended to the returned text.
/// - `@"path with spaces/file.txt"` supports quoted paths with spaces.
/// - `@path/to/image.png` is read as raw bytes and appended to the `images` list.
/// - `@dirname/` (trailing slash) includes all files in the directory (non-recursive).
/// - `@dirname/**/*.rs` expands glob patterns to matching files.
/// - `@"path with spaces/"` supports quoted directory paths.
/// - Paths are resolved relative to `cwd`.
/// - Directory and glob expansions are limited to 100 files per reference.
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
///
/// Dispatches to directory expansion, glob expansion, or single-file processing
/// based on the reference pattern.
fn process_file_ref(
    file_ref: &str,
    cwd: &Path,
    text_parts: &mut Vec<String>,
    images: &mut Vec<ImageAttachment>,
) -> anyhow::Result<()> {
    // Check if this is a directory reference (trailing slash)
    if file_ref.ends_with('/') || file_ref.ends_with('\\') {
        return process_directory_ref(file_ref, cwd, text_parts, images);
    }

    // Check if this is a glob pattern (contains * or ?)
    if file_ref.contains('*') || file_ref.contains('?') {
        return process_glob_ref(file_ref, cwd, text_parts, images);
    }

    // Otherwise, process as a single file
    process_single_file_ref(file_ref, cwd, text_parts, images)
}

/// Process a single file reference (no glob, no directory).
fn process_single_file_ref(
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

/// Expand a directory reference (trailing `/`) into individual file references.
///
/// Lists all files in the directory (non-recursive) and processes each one.
/// Limited to `MAX_EXPANSION_FILES` files.
fn process_directory_ref(
    file_ref: &str,
    cwd: &Path,
    text_parts: &mut Vec<String>,
    images: &mut Vec<ImageAttachment>,
) -> anyhow::Result<()> {
    let dir_path = resolve_file_path(file_ref, cwd);

    if !dir_path.is_dir() {
        anyhow::bail!("Directory not found: {}", file_ref);
    }

    let mut entries: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(&dir_path)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            entries.push(path);
        }
    }

    // Sort for deterministic output
    entries.sort();

    if entries.is_empty() {
        anyhow::bail!("Directory is empty: {}", file_ref);
    }

    if entries.len() > MAX_EXPANSION_FILES {
        anyhow::bail!(
            "Directory {} contains {} files, exceeding the limit of {}. Use a glob pattern to narrow the selection.",
            file_ref,
            entries.len(),
            MAX_EXPANSION_FILES
        );
    }

    for path in entries {
        // Build a display name relative to cwd
        let display_name = path
            .strip_prefix(cwd)
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();
        process_single_file_ref(&display_name, cwd, text_parts, images)?;
    }

    Ok(())
}

/// Expand a glob pattern into individual file references.
///
/// The pattern is resolved relative to `cwd`. Limited to `MAX_EXPANSION_FILES` files.
fn process_glob_ref(
    file_ref: &str,
    cwd: &Path,
    text_parts: &mut Vec<String>,
    images: &mut Vec<ImageAttachment>,
) -> anyhow::Result<()> {
    let pattern_path = if Path::new(file_ref).is_absolute() {
        file_ref.to_string()
    } else {
        cwd.join(file_ref).to_string_lossy().to_string()
    };

    let mut matched_files: Vec<PathBuf> = Vec::new();
    for entry in glob_match(&pattern_path)? {
        let path = entry?;
        if path.is_file() {
            matched_files.push(path);
        }
    }

    // Sort for deterministic output
    matched_files.sort();

    if matched_files.is_empty() {
        anyhow::bail!("No files matched pattern: {}", file_ref);
    }

    if matched_files.len() > MAX_EXPANSION_FILES {
        anyhow::bail!(
            "Glob pattern {} matched {} files, exceeding the limit of {}. Use a more specific pattern.",
            file_ref,
            matched_files.len(),
            MAX_EXPANSION_FILES
        );
    }

    for path in matched_files {
        let display_name = path
            .strip_prefix(cwd)
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();
        process_single_file_ref(&display_name, cwd, text_parts, images)?;
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

    // ---- Directory expansion tests ----

    #[test]
    fn test_directory_expansion_trailing_slash() {
        let tmp = TempDir::new().unwrap();
        let sub = tmp.path().join("src");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("a.rs"), "fn a() {}").unwrap();
        fs::write(sub.join("b.rs"), "fn b() {}").unwrap();
        fs::write(sub.join("c.txt"), "hello").unwrap();

        let result = process_input("review @src/", tmp.path()).unwrap();
        assert!(result.text.contains("fn a() {}"));
        assert!(result.text.contains("fn b() {}"));
        assert!(result.text.contains("hello"));
        // Should have 3 file tags
        assert_eq!(result.text.matches("<file name=").count(), 3);
    }

    #[test]
    fn test_directory_expansion_skips_subdirectories() {
        let tmp = TempDir::new().unwrap();
        let sub = tmp.path().join("mydir");
        fs::create_dir_all(sub.join("nested")).unwrap();
        fs::write(sub.join("top.txt"), "top level").unwrap();
        fs::write(sub.join("nested").join("deep.txt"), "deep level").unwrap();

        let result = process_input("@mydir/", tmp.path()).unwrap();
        // Only the top-level file should be included (non-recursive)
        assert!(result.text.contains("top level"));
        assert!(!result.text.contains("deep level"));
        assert_eq!(result.text.matches("<file name=").count(), 1);
    }

    #[test]
    fn test_directory_expansion_sorted() {
        let tmp = TempDir::new().unwrap();
        let sub = tmp.path().join("sorted");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("c.txt"), "c").unwrap();
        fs::write(sub.join("a.txt"), "a").unwrap();
        fs::write(sub.join("b.txt"), "b").unwrap();

        let result = process_input("@sorted/", tmp.path()).unwrap();
        // Files should appear in sorted order: a.txt, b.txt, c.txt
        let a_pos = result.text.find("a.txt").unwrap();
        let b_pos = result.text.find("b.txt").unwrap();
        let c_pos = result.text.find("c.txt").unwrap();
        assert!(a_pos < b_pos);
        assert!(b_pos < c_pos);
    }

    #[test]
    fn test_directory_nonexistent_kept_as_text() {
        let tmp = TempDir::new().unwrap();
        let result = process_input("@nodir/", tmp.path()).unwrap();
        assert!(result.text.contains("@nodir/"));
    }

    #[test]
    fn test_directory_empty_kept_as_text() {
        let tmp = TempDir::new().unwrap();
        let sub = tmp.path().join("emptydir");
        fs::create_dir(&sub).unwrap();

        let result = process_input("@emptydir/", tmp.path()).unwrap();
        // Empty directory should fail and be kept as literal text
        assert!(result.text.contains("@emptydir/"));
    }

    #[test]
    fn test_directory_with_images() {
        let tmp = TempDir::new().unwrap();
        let sub = tmp.path().join("assets");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("readme.txt"), "read me").unwrap();
        fs::write(sub.join("icon.png"), b"png data").unwrap();

        let result = process_input("@assets/", tmp.path()).unwrap();
        assert!(result.text.contains("read me"));
        assert_eq!(result.images.len(), 1);
        assert_eq!(result.images[0].data, b"png data");
    }

    // ---- Glob expansion tests ----

    #[test]
    fn test_glob_star_pattern() {
        let tmp = TempDir::new().unwrap();
        let sub = tmp.path().join("lib");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("foo.rs"), "fn foo() {}").unwrap();
        fs::write(sub.join("bar.rs"), "fn bar() {}").unwrap();
        fs::write(sub.join("data.txt"), "some data").unwrap();

        let result = process_input("@lib/*.rs", tmp.path()).unwrap();
        assert!(result.text.contains("fn foo() {}"));
        assert!(result.text.contains("fn bar() {}"));
        assert!(!result.text.contains("some data"));
        assert_eq!(result.text.matches("<file name=").count(), 2);
    }

    #[test]
    fn test_glob_recursive_pattern() {
        let tmp = TempDir::new().unwrap();
        let sub = tmp.path().join("project");
        fs::create_dir_all(sub.join("src").join("models")).unwrap();
        fs::write(sub.join("src").join("main.rs"), "fn main() {}").unwrap();
        fs::write(sub.join("src").join("models").join("user.rs"), "struct User;").unwrap();
        fs::write(sub.join("readme.txt"), "readme").unwrap();

        let result = process_input("@project/**/*.rs", tmp.path()).unwrap();
        assert!(result.text.contains("fn main() {}"));
        assert!(result.text.contains("struct User;"));
        assert!(!result.text.contains("readme"));
        assert_eq!(result.text.matches("<file name=").count(), 2);
    }

    #[test]
    fn test_glob_no_matches_kept_as_text() {
        let tmp = TempDir::new().unwrap();
        let result = process_input("@src/**/*.xyz", tmp.path()).unwrap();
        assert!(result.text.contains("@src/**/*.xyz"));
    }

    #[test]
    fn test_glob_question_mark_pattern() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("a1.txt"), "one").unwrap();
        fs::write(tmp.path().join("a2.txt"), "two").unwrap();
        fs::write(tmp.path().join("ab.txt"), "three").unwrap();

        let result = process_input("@a?.txt", tmp.path()).unwrap();
        assert!(result.text.contains("one"));
        assert!(result.text.contains("two"));
        assert!(result.text.contains("three"));
        assert_eq!(result.text.matches("<file name=").count(), 3);
    }

    #[test]
    fn test_glob_sorted_output() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("c.rs"), "c").unwrap();
        fs::write(tmp.path().join("a.rs"), "a").unwrap();
        fs::write(tmp.path().join("b.rs"), "b").unwrap();

        let result = process_input("@*.rs", tmp.path()).unwrap();
        let a_pos = result.text.find("a.rs").unwrap();
        let b_pos = result.text.find("b.rs").unwrap();
        let c_pos = result.text.find("c.rs").unwrap();
        assert!(a_pos < b_pos);
        assert!(b_pos < c_pos);
    }

    #[test]
    fn test_glob_with_images() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("a.png"), b"png a").unwrap();
        fs::write(tmp.path().join("b.png"), b"png b").unwrap();
        fs::write(tmp.path().join("c.txt"), "text c").unwrap();

        let result = process_input("@*.png", tmp.path()).unwrap();
        assert_eq!(result.images.len(), 2);
        assert!(result.text.contains("review") == false); // no text files matched
    }

    // ---- Quoted directory/glob tests ----

    #[test]
    fn test_quoted_directory_path() {
        let tmp = TempDir::new().unwrap();
        let sub = tmp.path().join("my dir");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("file.txt"), "inside spaced dir").unwrap();

        let result = process_input("check @\"my dir/\" please", tmp.path()).unwrap();
        assert!(result.text.contains("inside spaced dir"));
        assert!(result.text.contains("<file name="));
    }

    // ---- Max files limit tests ----

    #[test]
    fn test_directory_max_files_limit() {
        let tmp = TempDir::new().unwrap();
        let sub = tmp.path().join("bigdir");
        fs::create_dir(&sub).unwrap();

        // Create 101 files to exceed the limit
        for i in 0..101 {
            fs::write(sub.join(format!("file_{:03}.txt", i)), format!("content {}", i)).unwrap();
        }

        let result = process_input("@bigdir/", tmp.path()).unwrap();
        // Should fail and be kept as literal text due to exceeding MAX_EXPANSION_FILES
        assert!(result.text.contains("@bigdir/"));
    }

    #[test]
    fn test_glob_max_files_limit() {
        let tmp = TempDir::new().unwrap();
        let sub = tmp.path().join("manyfiles");
        fs::create_dir(&sub).unwrap();

        for i in 0..101 {
            fs::write(sub.join(format!("f_{:03}.txt", i)), format!("c {}", i)).unwrap();
        }

        let result = process_input("@manyfiles/*.txt", tmp.path()).unwrap();
        // Should fail and be kept as literal text
        assert!(result.text.contains("@manyfiles/*.txt"));
    }

    #[test]
    fn test_directory_exactly_at_limit() {
        let tmp = TempDir::new().unwrap();
        let sub = tmp.path().join("exactdir");
        fs::create_dir(&sub).unwrap();

        // Create exactly MAX_EXPANSION_FILES files (100)
        for i in 0..100 {
            fs::write(sub.join(format!("f_{:03}.txt", i)), format!("c {}", i)).unwrap();
        }

        let result = process_input("@exactdir/", tmp.path()).unwrap();
        // Should succeed since it's exactly at the limit
        assert_eq!(result.text.matches("<file name=").count(), 100);
    }

    // ---- Mixed usage tests ----

    #[test]
    fn test_mixed_file_and_directory() {
        let tmp = TempDir::new().unwrap();
        let sub = tmp.path().join("src");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("lib.rs"), "fn lib() {}").unwrap();
        fs::write(tmp.path().join("readme.txt"), "the readme").unwrap();

        let result = process_input("@readme.txt and @src/", tmp.path()).unwrap();
        assert!(result.text.contains("the readme"));
        assert!(result.text.contains("fn lib() {}"));
    }

    #[test]
    fn test_mixed_file_and_glob() {
        let tmp = TempDir::new().unwrap();
        let sub = tmp.path().join("src");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("main.rs"), "fn main() {}").unwrap();
        fs::write(sub.join("lib.rs"), "fn lib() {}").unwrap();
        fs::write(tmp.path().join("notes.txt"), "notes here").unwrap();

        let result = process_input("@notes.txt and @src/*.rs", tmp.path()).unwrap();
        assert!(result.text.contains("notes here"));
        assert!(result.text.contains("fn main() {}"));
        assert!(result.text.contains("fn lib() {}"));
    }
}
