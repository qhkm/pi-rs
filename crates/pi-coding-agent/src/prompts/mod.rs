use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

// ---------------------------------------------------------------------------
// PromptTemplate
// ---------------------------------------------------------------------------

/// A single prompt template loaded from a `.md` file with optional YAML
/// frontmatter.
///
/// File format:
///
/// ```text
/// ---
/// name: my-template
/// description: Does something useful
/// ---
/// Do {{action}} to {{target}}.
/// ```
///
/// The variables field contains every distinct `{{identifier}}` placeholder
/// found in the body, in the order of first appearance.
#[derive(Debug, Clone)]
pub struct PromptTemplate {
    /// The canonical name of this template (from frontmatter or file stem).
    pub name: String,
    /// Human-readable description extracted from frontmatter, if present.
    pub description: Option<String>,
    /// The raw body text (everything after the frontmatter delimiter).
    pub body: String,
    /// Ordered, deduplicated list of `{{variable}}` placeholders found in
    /// `body`.
    pub variables: Vec<String>,
    /// Absolute path of the source file.
    pub path: PathBuf,
}

impl PromptTemplate {
    /// Load a template from a file on disk.
    ///
    /// The function:
    /// 1. Reads the file.
    /// 2. Parses an optional `---` delimited YAML-lite frontmatter block for
    ///    `name:` and `description:` fields.
    /// 3. Treats everything after the frontmatter as the body.
    /// 4. Extracts `{{variable}}` placeholders from the body.
    ///
    /// If no `name:` is found in the frontmatter, the file stem is used.
    pub fn load(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read prompt template '{}'", path.display()))?;

        let (name, description, body) = parse_template_content(path, &raw);
        let variables = extract_variables(&body);

        Ok(Self {
            name,
            description,
            body,
            variables,
            path: path.to_path_buf(),
        })
    }
}

// ---------------------------------------------------------------------------
// PromptRegistry
// ---------------------------------------------------------------------------

/// A registry of named prompt templates.
///
/// Build it with [`PromptRegistry::new`] (empty) or by scanning directories
/// with [`PromptRegistry::discover`].
pub struct PromptRegistry {
    /// Templates keyed by their canonical name.
    templates: HashMap<String, PromptTemplate>,
}

impl PromptRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            templates: HashMap::new(),
        }
    }

    /// Scan `dirs` for `*.md` files, loading each as a [`PromptTemplate`].
    ///
    /// Directories are scanned in order; later directories override templates
    /// with the same name found in earlier directories. Non-existent or
    /// unreadable directories are silently skipped.
    pub fn discover(dirs: &[PathBuf]) -> Self {
        let mut registry = Self::new();

        for dir in dirs {
            if !dir.exists() {
                continue;
            }
            if let Ok(md_files) = collect_md_files(dir) {
                for file in md_files {
                    match PromptTemplate::load(&file) {
                        Ok(template) => {
                            registry.templates.insert(template.name.clone(), template);
                        }
                        Err(_) => {
                            // Skip files that cannot be parsed — keep going.
                        }
                    }
                }
            }
        }

        registry
    }

    /// Look up a template by its canonical name (case-sensitive).
    pub fn get(&self, name: &str) -> Option<&PromptTemplate> {
        self.templates.get(name)
    }

    /// Return all registered templates in arbitrary order.
    pub fn list(&self) -> Vec<&PromptTemplate> {
        self.templates.values().collect()
    }

    /// Return the number of registered templates.
    pub fn len(&self) -> usize {
        self.templates.len()
    }

    /// Returns `true` if the registry contains no templates.
    pub fn is_empty(&self) -> bool {
        self.templates.is_empty()
    }

    /// Insert or replace a template in the registry.
    pub fn insert(&mut self, template: PromptTemplate) {
        self.templates.insert(template.name.clone(), template);
    }
}

impl Default for PromptRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// expand
// ---------------------------------------------------------------------------

/// Expand a template body by replacing `{{key}}` placeholders with the
/// corresponding values from `vars`.
///
/// Placeholders whose names are not present in `vars` are left unchanged in
/// the output (i.e. they remain as `{{key}}`).
pub fn expand(template: &PromptTemplate, vars: &HashMap<String, String>) -> String {
    let mut result = template.body.clone();

    for (key, value) in vars {
        let placeholder = format!("{{{{{}}}}}", key);
        result = result.replace(&placeholder, value);
    }

    result
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Walk `root` (non-recursively, but does recurse into sub-directories) and
/// collect all `*.md` files sorted by path for deterministic ordering.
fn collect_md_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;

            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file() {
                let is_md = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.eq_ignore_ascii_case("md"))
                    .unwrap_or(false);
                if is_md {
                    files.push(path);
                }
            }
        }
    }

    files.sort();
    Ok(files)
}

/// Parse the raw file content into `(name, description, body)`.
///
/// Understands `---` delimited frontmatter containing simple `key: value`
/// lines.  Only `name` and `description` keys are consumed; all other lines
/// are ignored.  If the file has no valid frontmatter the entire content
/// becomes the body and the name falls back to the file stem.
fn parse_template_content(path: &Path, raw: &str) -> (String, Option<String>, String) {
    let mut name = String::new();
    let mut description: Option<String> = None;
    let mut body = raw.trim().to_string();

    if let Some(after_open) = raw.strip_prefix("---\n") {
        // Look for the closing "---" on its own line.
        if let Some(end_offset) = after_open.find("\n---\n") {
            let frontmatter = &after_open[..end_offset];
            // Everything after "\n---\n" (+ 5 chars for "\n---\n") is the body.
            body = after_open[(end_offset + 5)..].trim().to_string();

            for line in frontmatter.lines() {
                if let Some((k, v)) = line.split_once(':') {
                    let key = k.trim().to_lowercase();
                    let value = v.trim().trim_matches('"').trim_matches('\'').to_string();
                    match key.as_str() {
                        "name" => name = value,
                        "description" => {
                            if !value.is_empty() {
                                description = Some(value);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    // Fall back to the file stem when frontmatter provided no name.
    if name.is_empty() {
        name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unnamed-template")
            .to_string();
    }

    (name, description, body)
}

/// Extract all `{{identifier}}` variable names from `text`.
///
/// Returns a deduplicated list preserving first-appearance order.  Only names
/// that are purely alphanumeric-plus-underscore are recognised (matching the
/// typical template variable convention).
fn extract_variables(text: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut vars = Vec::new();

    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i + 3 < len {
        // Scan for '{{'
        if bytes[i] == b'{' && bytes[i + 1] == b'{' {
            let start = i + 2;
            let mut j = start;
            // Consume characters valid in a variable name.
            while j < len && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            // Check that we found at least one char and the placeholder closes.
            if j > start && j + 1 < len && bytes[j] == b'}' && bytes[j + 1] == b'}' {
                let var_name = &text[start..j];
                if seen.insert(var_name.to_string()) {
                    vars.push(var_name.to_string());
                }
                i = j + 2; // skip past '}}'
                continue;
            }
        }
        i += 1;
    }

    vars
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // -----------------------------------------------------------------------
    // Helper: write a file into `dir` with the given content, return the path.
    // -----------------------------------------------------------------------
    fn write_file(dir: &Path, filename: &str, content: &str) -> PathBuf {
        let path = dir.join(filename);
        fs::write(&path, content).unwrap();
        path
    }

    // -----------------------------------------------------------------------
    // Test 1 — load a template with YAML frontmatter
    // -----------------------------------------------------------------------
    #[test]
    fn test_load_with_frontmatter() {
        let tmp = TempDir::new().unwrap();
        let path = write_file(
            tmp.path(),
            "greet.md",
            "---\nname: greeting\ndescription: Greet someone warmly\n---\nHello, {{name}}! Welcome to {{place}}.",
        );

        let template = PromptTemplate::load(&path).unwrap();

        assert_eq!(template.name, "greeting");
        assert_eq!(
            template.description.as_deref(),
            Some("Greet someone warmly")
        );
        assert_eq!(template.body, "Hello, {{name}}! Welcome to {{place}}.");
        assert_eq!(template.variables, vec!["name", "place"]);
        assert_eq!(template.path, path);
    }

    // -----------------------------------------------------------------------
    // Test 2 — load a template without frontmatter (falls back to file stem)
    // -----------------------------------------------------------------------
    #[test]
    fn test_load_without_frontmatter_uses_file_stem() {
        let tmp = TempDir::new().unwrap();
        let path = write_file(
            tmp.path(),
            "summarize.md",
            "Summarize the following text in {{num_sentences}} sentences:\n\n{{text}}",
        );

        let template = PromptTemplate::load(&path).unwrap();

        assert_eq!(template.name, "summarize");
        assert!(template.description.is_none());
        assert!(template.body.contains("{{num_sentences}}"));
        assert!(template.body.contains("{{text}}"));
        // Both variables should be extracted.
        assert!(template.variables.contains(&"num_sentences".to_string()));
        assert!(template.variables.contains(&"text".to_string()));
    }

    // -----------------------------------------------------------------------
    // Test 3 — discover scans a directory and loads all *.md files
    // -----------------------------------------------------------------------
    #[test]
    fn test_discover_from_directory() {
        let tmp = TempDir::new().unwrap();

        write_file(
            tmp.path(),
            "alpha.md",
            "---\nname: alpha\ndescription: First template\n---\nHello {{user}}.",
        );
        write_file(
            tmp.path(),
            "beta.md",
            "---\nname: beta\ndescription: Second template\n---\nBye {{user}}.",
        );
        // A non-.md file should be ignored.
        write_file(tmp.path(), "ignored.txt", "This should not be loaded.");

        let registry = PromptRegistry::discover(&[tmp.path().to_path_buf()]);

        assert_eq!(registry.len(), 2);
        assert!(registry.get("alpha").is_some());
        assert!(registry.get("beta").is_some());
        assert!(registry.get("ignored").is_none());
    }

    // -----------------------------------------------------------------------
    // Test 4 — expand replaces known variables, leaves unknown ones intact
    // -----------------------------------------------------------------------
    #[test]
    fn test_expand_replaces_known_variables() {
        let tmp = TempDir::new().unwrap();
        let path = write_file(
            tmp.path(),
            "deploy.md",
            "Deploy {{service}} to {{environment}}. Notify {{owner}}.",
        );

        let template = PromptTemplate::load(&path).unwrap();

        let mut vars = HashMap::new();
        vars.insert("service".to_string(), "api-server".to_string());
        vars.insert("environment".to_string(), "production".to_string());
        // Deliberately omit "owner" to test the missing-variable behaviour.

        let expanded = expand(&template, &vars);

        assert_eq!(
            expanded,
            "Deploy api-server to production. Notify {{owner}}."
        );
    }

    // -----------------------------------------------------------------------
    // Test 5 — missing variables are left as-is in the output
    // -----------------------------------------------------------------------
    #[test]
    fn test_expand_missing_variables_left_as_is() {
        let tmp = TempDir::new().unwrap();
        let path = write_file(
            tmp.path(),
            "analyze.md",
            "Analyze {{file}} using {{tool}} with level {{depth}}.",
        );

        let template = PromptTemplate::load(&path).unwrap();

        // Provide only `tool`; the others must remain as placeholders.
        let mut vars = HashMap::new();
        vars.insert("tool".to_string(), "clippy".to_string());

        let expanded = expand(&template, &vars);

        assert!(expanded.contains("{{file}}"), "{{file}} should survive");
        assert!(expanded.contains("{{depth}}"), "{{depth}} should survive");
        assert!(expanded.contains("clippy"), "{{tool}} should be replaced");
        assert!(!expanded.contains("{{tool}}"), "{{tool}} must be gone");
    }

    // -----------------------------------------------------------------------
    // Test 6 — list returns all templates
    // -----------------------------------------------------------------------
    #[test]
    fn test_list_returns_all_templates() {
        let tmp = TempDir::new().unwrap();

        for name in &["one", "two", "three"] {
            write_file(
                tmp.path(),
                &format!("{}.md", name),
                &format!("---\nname: {}\n---\nBody for {}.", name, name),
            );
        }

        let registry = PromptRegistry::discover(&[tmp.path().to_path_buf()]);
        let mut listed_names: Vec<&str> = registry.list().iter().map(|t| t.name.as_str()).collect();
        listed_names.sort();

        assert_eq!(listed_names, vec!["one", "three", "two"]);
    }

    // -----------------------------------------------------------------------
    // Test 7 — later discover directory overrides earlier one for same name
    // -----------------------------------------------------------------------
    #[test]
    fn test_discover_later_dir_overrides_earlier() {
        let tmp = TempDir::new().unwrap();
        let dir_a = tmp.path().join("a");
        let dir_b = tmp.path().join("b");
        fs::create_dir_all(&dir_a).unwrap();
        fs::create_dir_all(&dir_b).unwrap();

        write_file(
            &dir_a,
            "common.md",
            "---\nname: common\ndescription: from A\n---\nVersion A.",
        );
        write_file(
            &dir_b,
            "common.md",
            "---\nname: common\ndescription: from B\n---\nVersion B.",
        );

        let registry = PromptRegistry::discover(&[dir_a, dir_b]);
        let template = registry.get("common").unwrap();

        assert_eq!(template.description.as_deref(), Some("from B"));
        assert!(template.body.contains("Version B"));
    }

    // -----------------------------------------------------------------------
    // Test 8 — duplicate {{var}} occurrences appear only once in variables
    // -----------------------------------------------------------------------
    #[test]
    fn test_extract_variables_deduplicates() {
        let tmp = TempDir::new().unwrap();
        let path = write_file(
            tmp.path(),
            "repeat.md",
            "Hello {{name}}, {{name}} again! And also {{other}}.",
        );

        let template = PromptTemplate::load(&path).unwrap();

        // "name" appears twice in the body but should be listed once.
        let name_count = template
            .variables
            .iter()
            .filter(|v| v.as_str() == "name")
            .count();
        assert_eq!(name_count, 1, "duplicate variable should appear only once");
        assert!(template.variables.contains(&"other".to_string()));
    }

    // -----------------------------------------------------------------------
    // Test 9 — empty directory yields an empty registry
    // -----------------------------------------------------------------------
    #[test]
    fn test_discover_empty_directory() {
        let tmp = TempDir::new().unwrap();
        let registry = PromptRegistry::discover(&[tmp.path().to_path_buf()]);
        assert!(registry.is_empty());
    }

    // -----------------------------------------------------------------------
    // Test 10 — non-existent directory is silently skipped
    // -----------------------------------------------------------------------
    #[test]
    fn test_discover_nonexistent_directory_is_skipped() {
        let missing = PathBuf::from("/this/path/does/not/exist");
        let registry = PromptRegistry::discover(&[missing]);
        assert!(registry.is_empty());
    }

    // -----------------------------------------------------------------------
    // Test 11 — template with no variables has an empty variables list
    // -----------------------------------------------------------------------
    #[test]
    fn test_template_with_no_variables() {
        let tmp = TempDir::new().unwrap();
        let path = write_file(
            tmp.path(),
            "static.md",
            "---\nname: static-prompt\n---\nPlease write idiomatic Rust code.",
        );

        let template = PromptTemplate::load(&path).unwrap();
        assert!(template.variables.is_empty());
    }

    // -----------------------------------------------------------------------
    // Test 12 — expand with no vars returns body unchanged
    // -----------------------------------------------------------------------
    #[test]
    fn test_expand_with_empty_vars_returns_body_unchanged() {
        let tmp = TempDir::new().unwrap();
        let path = write_file(
            tmp.path(),
            "fixed.md",
            "---\nname: fixed\n---\nFix {{issue}} in {{file}}.",
        );

        let template = PromptTemplate::load(&path).unwrap();
        let expanded = expand(&template, &HashMap::new());

        assert_eq!(expanded, "Fix {{issue}} in {{file}}.");
    }
}
