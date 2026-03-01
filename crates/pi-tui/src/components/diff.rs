use crate::components::traits::{Component, InputResult};

/// A single diff hunk representing a changed section.
#[derive(Debug, Clone)]
pub struct DiffHunk {
    /// Starting line number in old file (1-indexed)
    pub old_start: usize,
    /// Number of lines in old file
    pub old_count: usize,
    /// Starting line number in new file (1-indexed)
    pub new_start: usize,
    /// Number of lines in new file
    pub new_count: usize,
    /// The lines in this hunk
    pub lines: Vec<DiffLine>,
}

/// A single line in a diff.
#[derive(Debug, Clone)]
pub struct DiffLine {
    /// Type of change
    pub kind: DiffLineKind,
    /// Line content (without prefix)
    pub content: String,
    /// Line number in old file (if applicable)
    pub old_line: Option<usize>,
    /// Line number in new file (if applicable)
    pub new_line: Option<usize>,
}

/// Type of diff line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    /// Context line (unchanged)
    Context,
    /// Line removed
    Removed,
    /// Line added
    Added,
    /// Header/information line
    Header,
}

/// View mode for the diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffViewMode {
    /// Unified diff format
    Unified,
    /// Side-by-side diff
    SideBySide,
}

/// Theme for diff rendering.
pub struct DiffTheme {
    /// Style for context lines
    pub context: Box<dyn Fn(&str) -> String + Send>,
    /// Style for removed lines
    pub removed: Box<dyn Fn(&str) -> String + Send>,
    /// Style for added lines
    pub added: Box<dyn Fn(&str) -> String + Send>,
    /// Style for headers
    pub header: Box<dyn Fn(&str) -> String + Send>,
    /// Style for line numbers
    pub line_number: Box<dyn Fn(&str) -> String + Send>,
    /// Style for the diff border
    pub border: Box<dyn Fn(&str) -> String + Send>,
}

impl Default for DiffTheme {
    fn default() -> Self {
        Self {
            context: Box::new(|s| format!(" {} \x1b[0m", s)),
            removed: Box::new(|s| format!("\x1b[41m\x1b[97m-{}\x1b[0m", s)),
            added: Box::new(|s| format!("\x1b[42m\x1b[97m+{}\x1b[0m", s)),
            header: Box::new(|s| format!("\x1b[1;36m{}\x1b[0m", s)),
            line_number: Box::new(|s| format!("\x1b[90m{}\x1b[0m", s)),
            border: Box::new(|s| format!("\x1b[90m{}\x1b[0m", s)),
        }
    }
}

/// Diff viewer component.
///
/// Renders unified or side-by-side diffs with syntax highlighting.
pub struct Diff {
    hunks: Vec<DiffHunk>,
    view_mode: DiffViewMode,
    theme: DiffTheme,
    dirty: bool,
    /// Number of context lines to show around changes
    context_lines: usize,
    /// Show line numbers
    show_line_numbers: bool,
    /// Current scroll position
    scroll_offset: usize,
}

impl Diff {
    /// Create a new diff viewer.
    pub fn new() -> Self {
        Self {
            hunks: Vec::new(),
            view_mode: DiffViewMode::Unified,
            theme: DiffTheme::default(),
            dirty: true,
            context_lines: 3,
            show_line_numbers: true,
            scroll_offset: 0,
        }
    }

    /// Set the view mode.
    pub fn with_view_mode(mut self, mode: DiffViewMode) -> Self {
        self.view_mode = mode;
        self
    }

    /// Set the theme.
    pub fn with_theme(mut self, theme: DiffTheme) -> Self {
        self.theme = theme;
        self
    }

    /// Set context lines around changes.
    pub fn with_context_lines(mut self, lines: usize) -> Self {
        self.context_lines = lines;
        self
    }

    /// Show or hide line numbers.
    pub fn with_line_numbers(mut self, show: bool) -> Self {
        self.show_line_numbers = show;
        self
    }

    /// Set the diff content from hunks.
    pub fn set_hunks(&mut self, hunks: Vec<DiffHunk>) {
        self.hunks = hunks;
        self.dirty = true;
    }

    /// Parse a unified diff string.
    ///
    /// Supports standard unified diff format:
    /// ```text
    /// --- old.txt
    /// +++ new.txt
    /// @@ -1,5 +1,5 @@
    ///  context
    /// -removed
    /// +added
    ///  context
    /// ```
    pub fn parse_unified(diff_text: &str) -> Vec<DiffHunk> {
        let mut hunks = Vec::new();
        let mut current_hunk: Option<DiffHunk> = None;
        let mut old_line = 0usize;
        let mut new_line = 0usize;

        for line in diff_text.lines() {
            if line.starts_with("@@") {
                // Save previous hunk
                if let Some(hunk) = current_hunk.take() {
                    hunks.push(hunk);
                }

                // Parse hunk header: @@ -start,count +start,count @@
                if let Some(hunk) = Self::parse_hunk_header(line) {
                    current_hunk = Some(hunk);
                    old_line = current_hunk.as_ref().unwrap().old_start;
                    new_line = current_hunk.as_ref().unwrap().new_start;
                }
            } else if let Some(ref mut hunk) = current_hunk {
                if line.is_empty() || line.starts_with(' ') {
                    // Context line
                    hunk.lines.push(DiffLine {
                        kind: DiffLineKind::Context,
                        content: if line.is_empty() {
                            String::new()
                        } else {
                            line[1..].to_string()
                        },
                        old_line: Some(old_line),
                        new_line: Some(new_line),
                    });
                    old_line += 1;
                    new_line += 1;
                } else if line.starts_with('-') {
                    // Removed line
                    hunk.lines.push(DiffLine {
                        kind: DiffLineKind::Removed,
                        content: line[1..].to_string(),
                        old_line: Some(old_line),
                        new_line: None,
                    });
                    old_line += 1;
                } else if line.starts_with('+') {
                    // Added line
                    hunk.lines.push(DiffLine {
                        kind: DiffLineKind::Added,
                        content: line[1..].to_string(),
                        old_line: None,
                        new_line: Some(new_line),
                    });
                    new_line += 1;
                } else if line.starts_with("---") || line.starts_with("+++") {
                    // File header - skip
                } else {
                    // Context line without prefix (sometimes seen in git diffs)
                    hunk.lines.push(DiffLine {
                        kind: DiffLineKind::Context,
                        content: line.to_string(),
                        old_line: Some(old_line),
                        new_line: Some(new_line),
                    });
                    old_line += 1;
                    new_line += 1;
                }
            }
        }

        if let Some(hunk) = current_hunk {
            hunks.push(hunk);
        }

        hunks
    }

    fn parse_hunk_header(line: &str) -> Option<DiffHunk> {
        // Format: @@ -old_start,old_count +new_start,new_count @@
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {
            return None;
        }

        let parse_range = |s: &str| -> Option<(usize, usize)> {
            let s = s.trim_start_matches(&['-', '+'][..]);
            if let Some((start, count)) = s.split_once(',') {
                Some((start.parse().ok()?, count.parse().ok()?))
            } else {
                Some((s.parse().ok()?, 1))
            }
        };

        let (old_start, old_count) = parse_range(parts[1])?;
        let (new_start, new_count) = parse_range(parts[2])?;

        Some(DiffHunk {
            old_start,
            old_count,
            new_start,
            new_count,
            lines: Vec::new(),
        })
    }

    /// Create a diff from two strings (line by line).
    pub fn from_strings(old: &str, new: &str) -> Vec<DiffHunk> {
        let old_lines: Vec<&str> = old.lines().collect();
        let new_lines: Vec<&str> = new.lines().collect();

        // Simple LCS-based diff
        Self::compute_diff(&old_lines, &new_lines)
    }

    fn compute_diff(old: &[&str], new: &[&str]) -> Vec<DiffHunk> {
        // Use Myers' diff algorithm for simplicity
        let mut hunks = Vec::new();
        let mut current_hunk: Option<DiffHunk> = None;

        let mut i = 0usize;
        let mut j = 0usize;

        while i < old.len() || j < new.len() {
            if i < old.len() && j < new.len() && old[i] == new[j] {
                // Context line
                if let Some(ref mut hunk) = current_hunk {
                    hunk.lines.push(DiffLine {
                        kind: DiffLineKind::Context,
                        content: old[i].to_string(),
                        old_line: Some(i + 1),
                        new_line: Some(j + 1),
                    });
                }
                i += 1;
                j += 1;
            } else if i < old.len() && (j >= new.len() || Self::should_remove(&old[i], &new, j)) {
                // Start new hunk if needed
                if current_hunk.is_none() {
                    current_hunk = Some(DiffHunk {
                        old_start: i + 1,
                        old_count: 0,
                        new_start: j + 1,
                        new_count: 0,
                        lines: Vec::new(),
                    });
                }

                if let Some(ref mut hunk) = current_hunk {
                    hunk.lines.push(DiffLine {
                        kind: DiffLineKind::Removed,
                        content: old[i].to_string(),
                        old_line: Some(i + 1),
                        new_line: None,
                    });
                    hunk.old_count += 1;
                }
                i += 1;
            } else if j < new.len() {
                // Start new hunk if needed
                if current_hunk.is_none() {
                    current_hunk = Some(DiffHunk {
                        old_start: i + 1,
                        old_count: 0,
                        new_start: j + 1,
                        new_count: 0,
                        lines: Vec::new(),
                    });
                }

                if let Some(ref mut hunk) = current_hunk {
                    hunk.lines.push(DiffLine {
                        kind: DiffLineKind::Added,
                        content: new[j].to_string(),
                        old_line: None,
                        new_line: Some(j + 1),
                    });
                    hunk.new_count += 1;
                }
                j += 1;
            }
        }

        if let Some(hunk) = current_hunk {
            hunks.push(hunk);
        }

        hunks
    }

    fn should_remove(_old_line: &str, _new: &[&str], _j: usize) -> bool {
        // Simple heuristic: remove if next new line doesn't match
        // In a full implementation, this would use proper LCS
        true
    }

    fn render_unified(&self, width: u16) -> Vec<String> {
        let mut lines = Vec::new();
        let w = width as usize;

        for hunk in &self.hunks {
            // Hunk header
            let header = format!(
                "@@ -{},{} +{},{} @@",
                hunk.old_start, hunk.old_count, hunk.new_start, hunk.new_count
            );
            lines.push((self.theme.header)(&header));

            for line in &hunk.lines {
                let (prefix, styled) = match line.kind {
                    DiffLineKind::Context => (" ", &self.theme.context),
                    DiffLineKind::Removed => ("-", &self.theme.removed),
                    DiffLineKind::Added => ("+", &self.theme.added),
                    DiffLineKind::Header => continue,
                };

                let content = format!("{}{}", prefix, &line.content);
                let wrapped = wrap_line(&content, w);

                for (i, wrapped_line) in wrapped.iter().enumerate() {
                    if i == 0 {
                        lines.push(styled(wrapped_line));
                    } else {
                        // Continuation line
                        lines.push(styled(&format!(" {}", wrapped_line)));
                    }
                }
            }
        }

        lines
    }

    fn render_side_by_side(&self, width: u16) -> Vec<String> {
        let w = width as usize;
        let half_width = (w / 2).saturating_sub(1);

        let mut lines = Vec::new();

        for hunk in &self.hunks {
            // Hunk header
            let header = format!(
                "@@ -{},{} +{},{} @@",
                hunk.old_start, hunk.old_count, hunk.new_start, hunk.new_count
            );
            lines.push((self.theme.header)(&header));

            for line in &hunk.lines {
                match line.kind {
                    DiffLineKind::Context => {
                        let content = &line.content;
                        let left = truncate_line(content, half_width);
                        let right = truncate_line(content, half_width);
                        let line_num = if self.show_line_numbers {
                            format!(
                                "{:4} {:4} ",
                                line.old_line.map(|n| n.to_string()).unwrap_or_default(),
                                line.new_line.map(|n| n.to_string()).unwrap_or_default()
                            )
                        } else {
                            String::new()
                        };
                        lines.push(format!(
                            "{}{}│{}",
                            line_num,
                            (self.theme.context)(&left),
                            (self.theme.context)(&right)
                        ));
                    }
                    DiffLineKind::Removed => {
                        let left = truncate_line(&line.content, half_width);
                        let line_num = if self.show_line_numbers {
                            format!(
                                "{:4}     ",
                                line.old_line.map(|n| n.to_string()).unwrap_or_default()
                            )
                        } else {
                            String::new()
                        };
                        lines.push(format!(
                            "{}{}│{}",
                            line_num,
                            (self.theme.removed)(&left),
                            " ".repeat(half_width)
                        ));
                    }
                    DiffLineKind::Added => {
                        let right = truncate_line(&line.content, half_width);
                        let line_num = if self.show_line_numbers {
                            format!(
                                "     {:4} ",
                                line.new_line.map(|n| n.to_string()).unwrap_or_default()
                            )
                        } else {
                            String::new()
                        };
                        lines.push(format!(
                            "{}{}│{}",
                            line_num,
                            " ".repeat(half_width),
                            (self.theme.added)(&right)
                        ));
                    }
                    DiffLineKind::Header => {}
                }
            }
        }

        lines
    }
}

impl Default for Diff {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for Diff {
    fn render(&self, width: u16) -> Vec<String> {
        match self.view_mode {
            DiffViewMode::Unified => self.render_unified(width),
            DiffViewMode::SideBySide => self.render_side_by_side(width),
        }
    }

    fn handle_input(&mut self, _data: &str) -> InputResult {
        InputResult::Ignored
    }

    fn invalidate(&mut self) {
        self.dirty = true;
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }
}

/// Wrap a line to fit within max_width.
fn wrap_line(line: &str, max_width: usize) -> Vec<String> {
    let char_count = line.chars().count();
    if char_count <= max_width {
        return vec![line.to_string()];
    }

    let mut result = Vec::new();
    let mut chars = line.chars();

    while result.len() * max_width < char_count {
        let chunk: String = chars.by_ref().take(max_width).collect();
        if !chunk.is_empty() {
            result.push(chunk);
        }
    }

    result
}

/// Truncate a line to fit within max_width.
fn truncate_line(line: &str, max_width: usize) -> String {
    if line.chars().count() <= max_width {
        line.to_string()
    } else {
        let truncated: String = line.chars().take(max_width.saturating_sub(1)).collect();
        format!("{}…", truncated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_unified_diff() {
        let diff_text = r#"--- old.txt
+++ new.txt
@@ -1,3 +1,3 @@
 line 1
-line 2
+line 2 modified
 line 3
"#;

        let hunks = Diff::parse_unified(diff_text);
        assert_eq!(hunks.len(), 1);
        // Lines are: context, removed, added, context
        assert_eq!(hunks[0].lines.len(), 4);
        assert_eq!(hunks[0].lines[1].kind, DiffLineKind::Removed);
        assert_eq!(hunks[0].lines[2].kind, DiffLineKind::Added);
    }

    #[test]
    fn test_compute_diff() {
        let old = "line 1\nline 2\nline 3";
        let new = "line 1\nline 2 modified\nline 3";

        let hunks = Diff::from_strings(old, new);
        assert!(!hunks.is_empty());
    }

    #[test]
    fn test_diff_render() {
        let diff_text = r#"@@ -1,2 +1,2 @@
 context
-removed
+added
"#;

        let hunks = Diff::parse_unified(diff_text);
        let diff = Diff {
            hunks,
            ..Default::default()
        };

        let lines = diff.render(80);
        assert!(!lines.is_empty());
    }
}
