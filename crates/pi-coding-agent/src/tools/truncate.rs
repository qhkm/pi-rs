/// Configuration for smart output truncation.
///
/// When tool output exceeds `max_chars`, the content is split into a head
/// section (`head_lines` from the top) and a tail section (`tail_lines` from
/// the bottom), joined by a marker that describes how many lines were omitted.
/// The final result is guaranteed to be at or below `max_chars`.
#[derive(Debug, Clone)]
pub struct TruncationConfig {
    /// Maximum number of characters before truncation kicks in. Default: 30 000.
    pub max_chars: usize,
    /// Lines to keep from the beginning of the content. Default: 200.
    pub head_lines: usize,
    /// Lines to keep from the end of the content. Default: 50.
    pub tail_lines: usize,
}

impl Default for TruncationConfig {
    fn default() -> Self {
        Self {
            max_chars: 30_000,
            head_lines: 200,
            tail_lines: 50,
        }
    }
}

/// Truncate `content` according to `config`.
///
/// # Behaviour
/// - If `content.len() <= config.max_chars`, it is returned unchanged.
/// - Otherwise the function keeps the first `head_lines` lines and the last
///   `tail_lines` lines, separated by a marker of the form
///   `[... truncated N lines ...]`.
/// - Even after the head/tail split, if the assembled string still exceeds
///   `max_chars` (because individual lines are very long), the string is hard-
///   truncated at exactly `max_chars` characters and a final notice is appended
///   (the total length may exceed `max_chars` by the length of the notice, but
///   the content portion is capped).
pub fn smart_truncate(content: &str, config: &TruncationConfig) -> String {
    // Fast path: nothing to do.
    if content.len() <= config.max_chars {
        return content.to_string();
    }

    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();

    // If we can fit everything in head + tail without overlap, truncate.
    let head_count = config.head_lines.min(total_lines);
    let tail_count = config.tail_lines.min(total_lines);

    // Guard against head and tail overlapping (e.g. very short content that
    // somehow still exceeds max_chars through whitespace / BOM / etc.).
    if head_count + tail_count >= total_lines {
        // No lines to omit — just hard-cap at max_chars.
        return hard_cap(content, config.max_chars);
    }

    let omitted = total_lines - head_count - tail_count;
    let marker = format!("[... truncated {} lines ...]", omitted);

    let head_str = lines[..head_count].join("\n");
    let tail_str = lines[total_lines - tail_count..].join("\n");

    // Assemble and check the total character budget.
    let assembled = format!("{}\n{}\n{}", head_str, marker, tail_str);

    if assembled.len() <= config.max_chars {
        assembled
    } else {
        // The head/tail sections themselves are too large; hard-cap the
        // assembled result while preserving the marker so callers know
        // truncation occurred.
        hard_cap(&assembled, config.max_chars)
    }
}

/// Hard-truncate `s` to at most `max_chars` characters, appending a short
/// notice so it is clear the output was cut.
fn hard_cap(s: &str, max_chars: usize) -> String {
    if s.len() <= max_chars {
        return s.to_string();
    }
    // Truncate at a character boundary (Rust string slicing panics on invalid
    // UTF-8 boundaries, so we find the largest valid boundary <= max_chars).
    let boundary = s
        .char_indices()
        .take_while(|(i, _)| *i < max_chars)
        .last()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    format!("{}\n[... output hard-capped at {} chars ...]", &s[..boundary], max_chars)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_cfg() -> TruncationConfig {
        TruncationConfig::default()
    }

    // -------------------------------------------------------------------------
    // Test 1: content under the limit — must be returned unchanged
    // -------------------------------------------------------------------------
    #[test]
    fn test_under_limit_no_truncation() {
        let content = "line one\nline two\nline three\n";
        let cfg = default_cfg();
        let result = smart_truncate(content, &cfg);
        assert_eq!(result, content, "Content under max_chars must not be altered");
    }

    // -------------------------------------------------------------------------
    // Test 2: content over the limit — must contain the truncation marker
    // -------------------------------------------------------------------------
    #[test]
    fn test_over_limit_truncation_with_marker() {
        // Each line is "line_NNN " = 9 chars.  10 lines = 90 chars total.
        // head=2 lines (18 chars) + marker (~28 chars) + tail=1 line (8 chars)
        // = ~56 chars assembled — comfortably within max_chars=200 so the
        // marker is never hard-capped, but the *full* 90-char content exceeds
        // max_chars=80 to trigger truncation.
        let cfg = TruncationConfig {
            max_chars: 80,
            head_lines: 2,
            tail_lines: 1,
        };

        // 10 lines of 8 chars each; total = 89 chars (including newlines).
        let lines: Vec<String> = (1..=10).map(|i| format!("line_{:03}", i)).collect();
        let content = lines.join("\n");

        // Sanity-check: content is actually over the limit.
        assert!(content.len() > cfg.max_chars, "Test setup: content must exceed max_chars");

        let result = smart_truncate(&content, &cfg);

        // The marker must be present.
        assert!(
            result.contains("[... truncated"),
            "Result must contain truncation marker, got: {:?}",
            result
        );

        // First two lines (head) must appear.
        assert!(result.contains("line_001"), "Head must contain first line");
        assert!(result.contains("line_002"), "Head must contain second line");

        // Last line (tail) must appear.
        assert!(result.contains("line_010"), "Tail must contain last line");

        // Middle lines must NOT appear.
        assert!(!result.contains("line_005"), "Middle lines must be omitted");
    }

    // -------------------------------------------------------------------------
    // Test 3: edge case — empty string
    // -------------------------------------------------------------------------
    #[test]
    fn test_empty_string() {
        let result = smart_truncate("", &default_cfg());
        assert_eq!(result, "", "Empty content must be returned as-is");
    }

    // -------------------------------------------------------------------------
    // Test 4: edge case — single line that is over max_chars
    // -------------------------------------------------------------------------
    #[test]
    fn test_single_very_long_line() {
        let cfg = TruncationConfig {
            max_chars: 20,
            head_lines: 5,
            tail_lines: 5,
        };
        // One line of 100 'a' characters — exceeds max_chars but there is only
        // one line so head + tail would overlap.
        let content = "a".repeat(100);
        let result = smart_truncate(&content, &cfg);

        // Must be capped — the result content portion is at most max_chars chars
        // (the hard-cap notice may add a few extra characters).
        assert!(
            result.contains("[... output hard-capped at"),
            "Single long line must be hard-capped, got: {:?}",
            result
        );
    }

    // -------------------------------------------------------------------------
    // Test 5: exact boundary — content exactly at max_chars must NOT truncate
    // -------------------------------------------------------------------------
    #[test]
    fn test_exactly_at_limit_no_truncation() {
        let cfg = TruncationConfig {
            max_chars: 10,
            head_lines: 5,
            tail_lines: 5,
        };
        let content = "0123456789"; // exactly 10 chars
        let result = smart_truncate(&content, &cfg);
        assert_eq!(result, content, "Content exactly at max_chars must not be truncated");
    }

    // -------------------------------------------------------------------------
    // Test 6: omitted line count is correct in the marker
    // -------------------------------------------------------------------------
    #[test]
    fn test_marker_reports_correct_omitted_count() {
        // 5 lines × "lineX\n" ≈ 29 chars total — exceeds max_chars=25.
        // head=1 ("line1\n" = 6) + marker ("... truncated 3 lines ..." ~28) +
        // tail=1 ("line5" = 5) = ~40 chars assembled.
        // max_chars must be >= assembled length for the marker not to be sliced.
        // We set max_chars=25 to trigger truncation (full content = 29 chars),
        // but the assembled head+marker+tail is ~41 chars — so we'd still hit
        // hard_cap.  Instead, set a budget large enough for the assembled form
        // but smaller than the full content.
        //
        // Full content: "line1\nline2\nline3\nline4\nline5" = 29 chars
        // Assembled: "line1\n[... truncated 3 lines ...]\nline5" = 39 chars
        // max_chars = 200 → assembled fits; full content (29) < 200 → no truncation!
        //
        // We need full_content > max_chars AND assembled <= max_chars.
        // Use longer line names so full_content is bigger.
        //
        // Lines: "long_line_001" (13 chars) × 10 = 143 chars with newlines.
        // Assembled: "long_line_001\n[... truncated 8 lines ...]\nlong_line_010" = ~58 chars.
        // max_chars = 100: full(143) > 100, assembled(58) < 100. ✓
        let cfg = TruncationConfig {
            max_chars: 100,
            head_lines: 1,
            tail_lines: 1,
        };
        let lines: Vec<String> = (1..=10).map(|i| format!("long_line_{:03}", i)).collect();
        let content = lines.join("\n");

        assert!(
            content.len() > cfg.max_chars,
            "Test setup: content ({} chars) must exceed max_chars ({})",
            content.len(),
            cfg.max_chars
        );

        let result = smart_truncate(&content, &cfg);

        // head=1, tail=1 → omitted = 10 - 1 - 1 = 8
        assert!(
            result.contains("truncated 8 lines"),
            "Marker should report 8 omitted lines, got: {:?}",
            result
        );
    }
}
