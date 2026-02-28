use super::operations::{resolve_and_validate_path, FileOperations};
use async_trait::async_trait;
use pi_agent_core::{AgentTool, ToolContext, ToolResult};
use serde_json::Value;
use std::sync::Arc;

pub struct EditTool {
    ops: Arc<dyn FileOperations>,
}

impl EditTool {
    pub fn new(ops: Arc<dyn FileOperations>) -> Self {
        Self { ops }
    }
}

// ---------------------------------------------------------------------------
// Unified diff application
// ---------------------------------------------------------------------------

/// Summary of changes made when applying a diff.
#[derive(Debug, Default, PartialEq)]
pub struct DiffSummary {
    pub lines_added: usize,
    pub lines_removed: usize,
    pub hunks_applied: usize,
}

/// Parse and apply a unified diff to `original` text.
///
/// Supported diff format (standard unified diff):
///   ```text
///   @@ -old_start,old_count +new_start,new_count @@
///    context line
///   -removed line
///   +added line
///   ```
///
/// Rules:
/// - Lines starting with ` ` (space) are context lines – they must match the
///   original exactly (after stripping the leading space character).
/// - Lines starting with `-` are removed from the output.
/// - Lines starting with `+` are added to the output.
/// - Lines before the first `@@` header (file header lines such as `---` /
///   `+++`) are ignored.
/// - A trailing newline in the diff is preserved correctly: if the original
///   file does not end with `\n` and the diff adds one, the output will end
///   with `\n`, and vice-versa.
///
/// Returns the modified text and a [`DiffSummary`] on success, or an error
/// message describing the mismatch on failure.
pub fn apply_unified_diff(original: &str, diff: &str) -> Result<(String, DiffSummary), String> {
    // Split original into lines, keeping the newline terminators attached so
    // we can reconstruct the file faithfully.
    let orig_lines: Vec<&str> = split_lines_keep_terminator(original);

    // Collect hunks from the diff text.
    let hunks = parse_hunks(diff)?;

    if hunks.is_empty() {
        return Err("diff contains no hunks".to_string());
    }

    let mut summary = DiffSummary::default();
    // We build the output by consuming orig_lines from front to back.
    let mut out: Vec<String> = Vec::with_capacity(orig_lines.len());
    // `orig_pos` is a 0-based index into orig_lines (the "old" side).
    let mut orig_pos: usize = 0;

    for hunk in &hunks {
        // The hunk header gives us the 1-based start line in the old file.
        let hunk_old_start = hunk.old_start; // 1-based

        // Copy any unchanged lines that come before this hunk.
        let copy_until = hunk_old_start.saturating_sub(1); // convert to 0-based exclusive
        while orig_pos < copy_until {
            if orig_pos >= orig_lines.len() {
                return Err(format!(
                    "hunk at old line {} references line {} which is past end of file ({} lines)",
                    hunk_old_start,
                    orig_pos + 1,
                    orig_lines.len()
                ));
            }
            out.push(orig_lines[orig_pos].to_string());
            orig_pos += 1;
        }

        // Apply the hunk lines.
        for line in &hunk.lines {
            match line.kind {
                HunkLineKind::Context => {
                    // Verify the context line matches the original.
                    if orig_pos >= orig_lines.len() {
                        return Err(format!(
                            "context mismatch: expected {:?} but reached end of file",
                            line.content
                        ));
                    }
                    let orig = strip_terminator(orig_lines[orig_pos]);
                    if orig != line.content {
                        return Err(format!(
                            "context mismatch at original line {}: expected {:?}, got {:?}",
                            orig_pos + 1,
                            line.content,
                            orig
                        ));
                    }
                    // Keep context line (use original's line-ending).
                    out.push(orig_lines[orig_pos].to_string());
                    orig_pos += 1;
                }
                HunkLineKind::Remove => {
                    // Verify the line we are removing matches the original.
                    if orig_pos >= orig_lines.len() {
                        return Err(format!(
                            "remove mismatch: expected {:?} but reached end of file",
                            line.content
                        ));
                    }
                    let orig = strip_terminator(orig_lines[orig_pos]);
                    if orig != line.content {
                        return Err(format!(
                            "remove mismatch at original line {}: expected {:?}, got {:?}",
                            orig_pos + 1,
                            line.content,
                            orig
                        ));
                    }
                    // Skip (do not push to output).
                    orig_pos += 1;
                    summary.lines_removed += 1;
                }
                HunkLineKind::Add => {
                    // Insert new line. We use `\n` as the terminator unless
                    // the content already includes one.
                    let mut s = line.content.clone();
                    if !s.ends_with('\n') && !s.ends_with('\r') {
                        s.push('\n');
                    }
                    out.push(s);
                    summary.lines_added += 1;
                }
            }
        }

        summary.hunks_applied += 1;
    }

    // Copy any remaining original lines after the last hunk.
    while orig_pos < orig_lines.len() {
        out.push(orig_lines[orig_pos].to_string());
        orig_pos += 1;
    }

    // Reconstruct the file text.
    // `out.join("")` concatenates with no separator, faithfully reproducing
    // the original line terminators.  Add lines always carry a `\n` (added
    // above), and context/unchanged lines are taken verbatim from orig_lines.
    // No further trailing-newline fixup is required.
    let result = out.join("");

    Ok((result, summary))
}

// ---------------------------------------------------------------------------
// Internal parsing helpers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
enum HunkLineKind {
    Context,
    Remove,
    Add,
}

#[derive(Debug)]
struct HunkLine {
    kind: HunkLineKind,
    /// Line content without the leading `+`/`-`/` ` prefix and without the
    /// line terminator.
    content: String,
}

#[derive(Debug)]
struct Hunk {
    /// 1-based start line in the original ("old") file.
    old_start: usize,
    /// Expected number of lines from the old file this hunk covers.
    old_count: usize,
    lines: Vec<HunkLine>,
}

/// Split text into lines while keeping the `\n` (or `\r\n`) terminator
/// attached to each element so we can reconstruct the file exactly.
fn split_lines_keep_terminator(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (i, c) in text.char_indices() {
        if c == '\n' {
            lines.push(&text[start..=i]);
            start = i + 1;
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

/// Strip a trailing `\n` or `\r\n` from a line slice.
fn strip_terminator(line: &str) -> &str {
    line.trim_end_matches('\n').trim_end_matches('\r')
}

/// Parse all hunks out of a unified diff string.
fn parse_hunks(diff: &str) -> Result<Vec<Hunk>, String> {
    let mut hunks: Vec<Hunk> = Vec::new();
    let mut current: Option<Hunk> = None;

    for raw_line in diff.lines() {
        if raw_line.starts_with("@@") {
            // Finish the previous hunk.
            if let Some(h) = current.take() {
                validate_hunk(&h)?;
                hunks.push(h);
            }
            // Parse: `@@ -old_start[,old_count] +new_start[,new_count] @@`
            let (old_start, old_count) = parse_hunk_header(raw_line)?;
            current = Some(Hunk {
                old_start,
                old_count,
                lines: Vec::new(),
            });
        } else if let Some(ref mut hunk) = current {
            let kind = if raw_line.starts_with('+') {
                HunkLineKind::Add
            } else if raw_line.starts_with('-') {
                HunkLineKind::Remove
            } else if raw_line.starts_with(' ') {
                HunkLineKind::Context
            } else {
                // Lines like `\ No newline at end of file` – skip silently.
                continue;
            };
            let content = raw_line[1..].to_string();
            hunk.lines.push(HunkLine { kind, content });
        }
        // Lines before the first `@@` (e.g. `--- a/file`, `+++ b/file`) are
        // ignored – fall through.
    }

    // Don't forget the last hunk.
    if let Some(h) = current.take() {
        validate_hunk(&h)?;
        hunks.push(h);
    }

    Ok(hunks)
}

/// Parse the `@@ -old_start[,old_count] +new_start[,new_count] @@` header.
/// Returns `(old_start, old_count)`.
fn parse_hunk_header(line: &str) -> Result<(usize, usize), String> {
    // Example: `@@ -10,5 +10,7 @@` or `@@ -10 +10,7 @@`
    let err = || format!("malformed hunk header: {:?}", line);

    // Find the content between the first `@@` and the second `@@`.
    let inner = line.trim_start_matches('@').trim_start_matches(' ');
    // inner now looks like `-10,5 +10,7 @@` or similar.

    // Split on whitespace, grab the part starting with `-`.
    let old_part = inner
        .split_whitespace()
        .find(|s| s.starts_with('-'))
        .ok_or_else(err)?;

    let old_str = old_part.trim_start_matches('-');
    if old_str.contains(',') {
        let mut it = old_str.splitn(2, ',');
        let start: usize = it.next().unwrap().parse().map_err(|_| err())?;
        let count: usize = it.next().unwrap().parse().map_err(|_| err())?;
        Ok((start, count))
    } else {
        let start: usize = old_str.parse().map_err(|_| err())?;
        Ok((start, 1))
    }
}

/// Basic sanity check: the number of context+remove lines in the hunk should
/// equal old_count (when old_count > 0).
fn validate_hunk(hunk: &Hunk) -> Result<(), String> {
    let actual_old = hunk
        .lines
        .iter()
        .filter(|l| matches!(l.kind, HunkLineKind::Context | HunkLineKind::Remove))
        .count();

    // old_count == 0 is valid (pure-insertion hunk).
    if hunk.old_count > 0 && actual_old != hunk.old_count {
        return Err(format!(
            "hunk at old line {} declares old_count={} but has {} context/remove lines",
            hunk.old_start, hunk.old_count, actual_old
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// AgentTool implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl AgentTool for EditTool {
    fn name(&self) -> &str {
        "edit"
    }

    fn description(&self) -> &str {
        "Edit a file using either exact string replacement (old_text/new_text) or a unified diff \
         patch (diff). The two modes are mutually exclusive: provide either diff alone or both \
         old_text and new_text."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path to edit"
                },
                "old_text": {
                    "type": "string",
                    "description": "(string-replace mode) Exact text to find and replace. \
                                    Must match exactly one location in the file."
                },
                "new_text": {
                    "type": "string",
                    "description": "(string-replace mode) Replacement text."
                },
                "diff": {
                    "type": "string",
                    "description": "(diff mode) A unified diff patch to apply to the file. \
                                    Use standard unified diff format with @@ hunk headers, \
                                    '+' for added lines, '-' for removed lines, and ' ' \
                                    (space) for context lines. When this parameter is \
                                    provided, old_text and new_text must not be supplied."
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> pi_agent_core::Result<ToolResult> {
        let path_str = args.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
            pi_agent_core::AgentError::ToolValidation {
                tool_name: "edit".into(),
                message: "missing 'path'".into(),
            }
        })?;

        let path = resolve_and_validate_path(&ctx.cwd, path_str).map_err(|msg| {
            pi_agent_core::AgentError::ToolValidation {
                tool_name: "edit".into(),
                message: msg,
            }
        })?;

        // Determine mode: diff takes precedence; fall back to old_text/new_text.
        let diff_param = args.get("diff").and_then(|v| v.as_str());
        let old_text = args.get("old_text").and_then(|v| v.as_str());
        let new_text = args.get("new_text").and_then(|v| v.as_str());

        if diff_param.is_some() && (old_text.is_some() || new_text.is_some()) {
            return Ok(ToolResult::error(
                "Provide either 'diff' or 'old_text'/'new_text', not both.",
            ));
        }

        if let Some(diff_str) = diff_param {
            // ----- diff mode -----
            let data = self.ops.read_file(&path).await.map_err(|e| {
                pi_agent_core::AgentError::ToolExecution {
                    tool_name: "edit".into(),
                    message: format!("read {}: {}", path.display(), e),
                }
            })?;
            let content = String::from_utf8_lossy(&data).to_string();

            match apply_unified_diff(&content, diff_str) {
                Ok((new_content, summary)) => {
                    self.ops
                        .write_file(&path, new_content.as_bytes())
                        .await
                        .map_err(|e| pi_agent_core::AgentError::ToolExecution {
                            tool_name: "edit".into(),
                            message: format!("write {}: {}", path.display(), e),
                        })?;

                    Ok(ToolResult::success(format!(
                        "Applied {} hunk(s) to {}: +{} line(s), -{} line(s)",
                        summary.hunks_applied,
                        path.display(),
                        summary.lines_added,
                        summary.lines_removed,
                    )))
                }
                Err(msg) => Ok(ToolResult::error(format!("diff apply failed: {}", msg))),
            }
        } else {
            // ----- string-replace mode -----
            let old_text = old_text.ok_or_else(|| pi_agent_core::AgentError::ToolValidation {
                tool_name: "edit".into(),
                message: "missing 'old_text' (required when 'diff' is not provided)".into(),
            })?;
            let new_text = new_text.ok_or_else(|| pi_agent_core::AgentError::ToolValidation {
                tool_name: "edit".into(),
                message: "missing 'new_text' (required when 'diff' is not provided)".into(),
            })?;

            let data = self.ops.read_file(&path).await.map_err(|e| {
                pi_agent_core::AgentError::ToolExecution {
                    tool_name: "edit".into(),
                    message: format!("read {}: {}", path.display(), e),
                }
            })?;
            let content = String::from_utf8_lossy(&data).to_string();

            let matches: Vec<_> = content.match_indices(old_text).collect();
            if matches.is_empty() {
                return Ok(ToolResult::error(format!(
                    "old_text not found in {}. Make sure it matches exactly.",
                    path.display()
                )));
            }
            if matches.len() > 1 {
                return Ok(ToolResult::error(format!(
                    "old_text matches {} locations in {}. Provide more context to make it unique.",
                    matches.len(),
                    path.display()
                )));
            }

            let new_content = content.replacen(old_text, new_text, 1);
            self.ops
                .write_file(&path, new_content.as_bytes())
                .await
                .map_err(|e| pi_agent_core::AgentError::ToolExecution {
                    tool_name: "edit".into(),
                    message: format!("write {}: {}", path.display(), e),
                })?;

            let line_num = content[..matches[0].0].lines().count() + 1;
            Ok(ToolResult::success(format!(
                "Edited {} at line {}",
                path.display(),
                line_num
            )))
        }
    }
    
    fn clone_boxed(&self) -> Box<dyn AgentTool> {
        Box::new(EditTool { ops: self.ops.clone() })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::{apply_unified_diff, DiffSummary};

    // -----------------------------------------------------------------------
    // Helper: build a diff string from hunks, mimicking `diff -u` output.
    // -----------------------------------------------------------------------

    fn make_diff(hunks: &[&str]) -> String {
        hunks.join("\n")
    }

    // -----------------------------------------------------------------------
    // 1. Simple add – insert a new line between two existing lines.
    // -----------------------------------------------------------------------

    #[test]
    fn simple_add_inserts_line() {
        let original = "line one\nline two\nline three\n";
        let diff = make_diff(&[
            "@@ -1,3 +1,4 @@",
            " line one",
            "+inserted line",
            " line two",
            " line three",
        ]);

        let (result, summary) = apply_unified_diff(original, &diff).expect("apply failed");

        assert_eq!(result, "line one\ninserted line\nline two\nline three\n");
        assert_eq!(
            summary,
            DiffSummary {
                lines_added: 1,
                lines_removed: 0,
                hunks_applied: 1,
            }
        );
    }

    // -----------------------------------------------------------------------
    // 2. Simple remove – delete one line.
    // -----------------------------------------------------------------------

    #[test]
    fn simple_remove_deletes_line() {
        let original = "alpha\nbeta\ngamma\n";
        let diff = make_diff(&[
            "@@ -1,3 +1,2 @@",
            " alpha",
            "-beta",
            " gamma",
        ]);

        let (result, summary) = apply_unified_diff(original, &diff).expect("apply failed");

        assert_eq!(result, "alpha\ngamma\n");
        assert_eq!(
            summary,
            DiffSummary {
                lines_added: 0,
                lines_removed: 1,
                hunks_applied: 1,
            }
        );
    }

    // -----------------------------------------------------------------------
    // 3. Replace – remove one line and add another in its place.
    // -----------------------------------------------------------------------

    #[test]
    fn replace_substitutes_line() {
        let original = "fn old_name() {\n    // body\n}\n";
        let diff = make_diff(&[
            "@@ -1,3 +1,3 @@",
            "-fn old_name() {",
            "+fn new_name() {",
            "     // body",
            " }",
        ]);

        let (result, summary) = apply_unified_diff(original, &diff).expect("apply failed");

        assert_eq!(result, "fn new_name() {\n    // body\n}\n");
        assert_eq!(
            summary,
            DiffSummary {
                lines_added: 1,
                lines_removed: 1,
                hunks_applied: 1,
            }
        );
    }

    // -----------------------------------------------------------------------
    // 4. Multi-hunk diff – two separate edits in the same file.
    // -----------------------------------------------------------------------

    #[test]
    fn multi_hunk_applies_both_changes() {
        // 10-line file.
        let original = (1..=10)
            .map(|i| format!("line {}\n", i))
            .collect::<String>();

        let diff = make_diff(&[
            // First hunk: replace line 2
            "@@ -2,1 +2,1 @@",
            "-line 2",
            "+LINE TWO",
            // Second hunk: add a line after line 8
            "@@ -8,2 +8,3 @@",
            " line 8",
            "+extra line",
            " line 9",
        ]);

        let (result, summary) = apply_unified_diff(&original, &diff).expect("apply failed");

        let expected = "line 1\nLINE TWO\nline 3\nline 4\nline 5\nline 6\nline 7\nline 8\nextra line\nline 9\nline 10\n";
        assert_eq!(result, expected);
        assert_eq!(
            summary,
            DiffSummary {
                lines_added: 2,  // "LINE TWO" + "extra line"
                lines_removed: 1, // "line 2"
                hunks_applied: 2,
            }
        );
    }

    // -----------------------------------------------------------------------
    // 5. Context mismatch returns an error.
    // -----------------------------------------------------------------------

    #[test]
    fn context_mismatch_returns_error() {
        let original = "foo\nbar\nbaz\n";
        // The diff claims the context line is "WRONG" but the file has "bar".
        let diff = make_diff(&[
            "@@ -1,3 +1,3 @@",
            " foo",
            " WRONG",   // <-- mismatch
            " baz",
        ]);

        let err = apply_unified_diff(original, &diff).expect_err("should have failed");
        assert!(
            err.contains("context mismatch"),
            "error should mention context mismatch, got: {err}"
        );
    }

    // -----------------------------------------------------------------------
    // 6. Remove mismatch returns an error.
    // -----------------------------------------------------------------------

    #[test]
    fn remove_mismatch_returns_error() {
        let original = "hello\nworld\n";
        // Diff claims to remove "NONEXISTENT" but file has "hello".
        let diff = make_diff(&[
            "@@ -1,2 +1,1 @@",
            "-NONEXISTENT",
            " world",
        ]);

        let err = apply_unified_diff(original, &diff).expect_err("should have failed");
        assert!(
            err.contains("remove mismatch"),
            "error should mention remove mismatch, got: {err}"
        );
    }

    // -----------------------------------------------------------------------
    // 7. Hunk at end of file (no trailing newline in original).
    // -----------------------------------------------------------------------

    #[test]
    fn diff_on_file_without_trailing_newline() {
        let original = "a\nb\nc"; // no trailing \n
        let diff = make_diff(&[
            "@@ -2,1 +2,1 @@",
            "-b",
            "+B",
        ]);

        let (result, summary) = apply_unified_diff(original, &diff).expect("apply failed");

        // The replacement line gets a \n appended because we always add one
        // for Add lines. The final line "c" has no \n (it was stored without
        // one).
        assert!(result.contains("B\n"), "result should contain replaced line");
        assert!(result.contains('c'), "result should still contain 'c'");
        assert_eq!(
            summary,
            DiffSummary {
                lines_added: 1,
                lines_removed: 1,
                hunks_applied: 1,
            }
        );
    }

    // -----------------------------------------------------------------------
    // 8. Empty diff (no @@ headers) returns an error.
    // -----------------------------------------------------------------------

    #[test]
    fn empty_diff_returns_error() {
        let original = "some content\n";
        let diff = "--- a/file\n+++ b/file\n"; // header lines only, no hunk
        let err = apply_unified_diff(original, diff).expect_err("should fail on empty diff");
        assert!(
            err.contains("no hunks"),
            "error should mention missing hunks, got: {err}"
        );
    }

    // -----------------------------------------------------------------------
    // 9. Pure-insertion hunk at start of file (old_count = 0).
    // -----------------------------------------------------------------------

    #[test]
    fn pure_insertion_hunk_at_start() {
        let original = "existing\n";
        // @@ -0,0 +1,2 @@ means "insert 2 lines before the first line"
        let diff = "@@ -0,0 +1,2 @@\n+first\n+second\n";

        let (result, summary) = apply_unified_diff(original, diff).expect("apply failed");

        assert_eq!(result, "first\nsecond\nexisting\n");
        assert_eq!(
            summary,
            DiffSummary {
                lines_added: 2,
                lines_removed: 0,
                hunks_applied: 1,
            }
        );
    }

    // -----------------------------------------------------------------------
    // 10. Diff with standard file headers is handled correctly.
    // -----------------------------------------------------------------------

    #[test]
    fn diff_with_file_headers_ignored() {
        let original = "x\ny\n";
        let diff = "--- a/file.txt\n+++ b/file.txt\n@@ -1,2 +1,2 @@\n-x\n+X\n y\n";

        let (result, summary) = apply_unified_diff(original, diff).expect("apply failed");

        assert_eq!(result, "X\ny\n");
        assert_eq!(
            summary,
            DiffSummary {
                lines_added: 1,
                lines_removed: 1,
                hunks_applied: 1,
            }
        );
    }
}
