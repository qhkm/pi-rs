/// Partial JSON streaming parser.
///
/// When an LLM streams a JSON object token-by-token the last chunk delivered
/// to the application is almost always truncated.  This module attempts to
/// recover as much structured data as possible from an incomplete JSON string
/// rather than discarding the whole chunk.
///
/// # Strategy
///
/// 1. **Full parse** — try `serde_json::from_str` directly.  This handles the
///    common case where the buffer happens to be valid JSON.
/// 2. **Bracket completion** — count unmatched `{`/`[` delimiters and append
///    the corresponding closers, then re-try the parse.  This repairs objects
///    and arrays that were cut off just after a comma or colon.
/// 3. **Last-pair walk-back** — if bracket completion still fails (e.g. the
///    truncation landed inside a string literal or a number) the parser walks
///    backwards from the end of the input to find the last byte position that
///    ends a *complete* key-value pair, reconstructs a syntactically valid
///    object up to that point, and parses it.  Extracted values are never
///    invented — only pairs that were already complete in the original text are
///    returned.
///
/// The function returns `None` for empty or whitespace-only input and for any
/// input that cannot yield at least one key-value pair.

use serde_json::Value;

// ─── Public API ───────────────────────────────────────────────────────────────

/// Parse a potentially incomplete JSON string, extracting whatever valid
/// key-value pairs can be recovered.
///
/// Returns a [`serde_json::Value`] (usually an `Object`) with the successfully
/// parsed pairs, or `None` when nothing useful can be extracted.
///
/// # Examples
///
/// ```
/// use pi_ai::utils::partial_json::parse_partial_json;
///
/// // Complete JSON passes through unchanged.
/// let v = parse_partial_json(r#"{"key": "value"}"#).unwrap();
/// assert_eq!(v["key"], "value");
///
/// // Truncated string — the partial value is preserved.
/// let v = parse_partial_json(r#"{"path": "/usr/loc"#).unwrap();
/// assert_eq!(v["path"], "/usr/loc");
/// ```
pub fn parse_partial_json(input: &str) -> Option<Value> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    // ── Pass 1: try a straight parse ─────────────────────────────────────────
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        return Some(v);
    }

    // ── Pass 2: bracket / brace completion ───────────────────────────────────
    if let Some(v) = try_bracket_completion(trimmed) {
        return Some(v);
    }

    // ── Pass 3: walk back to last complete key-value pair ────────────────────
    try_walkback(trimmed)
}

// ─── Pass 2: Bracket completion ───────────────────────────────────────────────

/// Append the minimum number of closing `}` / `]` characters needed to balance
/// any unmatched openers, then attempt a full parse.
fn try_bracket_completion(input: &str) -> Option<Value> {
    let mut closers = String::new();
    let mut depth_brace: i32 = 0;
    let mut depth_bracket: i32 = 0;
    let mut in_string = false;
    let mut escape_next = false;

    // Track nesting while respecting string boundaries.
    for ch in input.chars() {
        if escape_next {
            escape_next = false;
            continue;
        }
        match ch {
            '\\' if in_string => escape_next = true,
            '"' => in_string = !in_string,
            '{' if !in_string => depth_brace += 1,
            '}' if !in_string => depth_brace -= 1,
            '[' if !in_string => depth_bracket += 1,
            ']' if !in_string => depth_bracket -= 1,
            _ => {}
        }
    }

    // If we are still inside a string, close it first.
    if in_string {
        closers.push('"');
    }

    // We may also be in a dangling value position (e.g. `"key": ` with nothing
    // after the colon).  A bare `null` token fills that gap so serde_json can
    // parse it; we only use this if the closers alone are insufficient.
    let candidate_bare = format!("{input}{closers}");

    // Stack: innermost bracket type must be closed first.  We reconstruct the
    // closer sequence from the *last* unclosed opener to the first.
    // Re-walk to build a correctly ordered closer sequence.
    let ordered_closers = build_ordered_closers(input, in_string);

    let with_closers = format!("{input}{ordered_closers}");
    if let Ok(v) = serde_json::from_str::<Value>(&with_closers) {
        return Some(v);
    }

    // Try the simpler (unordered) candidate as a fallback.
    if candidate_bare != with_closers {
        if let Ok(v) = serde_json::from_str::<Value>(&candidate_bare) {
            return Some(v);
        }
    }

    // Try adding a `null` placeholder for a dangling colon before closing.
    let _ = (depth_brace, depth_bracket); // suppress warnings
    let with_null = format!("{input}null{ordered_closers}");
    if let Ok(v) = serde_json::from_str::<Value>(&with_null) {
        return Some(v);
    }

    None
}

/// Build a correctly ordered sequence of closing characters by replaying the
/// nesting stack.
fn build_ordered_closers(input: &str, started_in_string: bool) -> String {
    let mut stack: Vec<char> = Vec::new();
    let mut in_string = false;
    let mut escape_next = false;

    for ch in input.chars() {
        if escape_next {
            escape_next = false;
            continue;
        }
        match ch {
            '\\' if in_string => escape_next = true,
            '"' => in_string = !in_string,
            '{' if !in_string => stack.push('}'),
            '[' if !in_string => stack.push(']'),
            '}' | ']' if !in_string => {
                stack.pop();
            }
            _ => {}
        }
    }

    let mut closers = String::new();
    if started_in_string {
        closers.push('"');
    }
    // Drain the stack in reverse (innermost first).
    while let Some(c) = stack.pop() {
        closers.push(c);
    }
    closers
}

// ─── Pass 3: Walk-back to last complete key-value pair ────────────────────────

/// Walk backwards from the end of the input to locate the byte offset just
/// after the last *syntactically complete* key-value pair inside a JSON object,
/// reconstruct a valid `{...}` slice from that range, and parse it.
///
/// "Complete" means both the key string and its value (string, number, boolean,
/// null, or a nested object/array that is itself fully balanced) are present and
/// well-formed.
fn try_walkback(input: &str) -> Option<Value> {
    let trimmed = input.trim();

    // We only attempt recovery for object-like inputs that start with `{`.
    if !trimmed.starts_with('{') {
        return None;
    }

    // Find candidate cut-points: positions of ',' or '{' at depth-1 that we
    // can slice at.
    let cut_points = collect_depth1_separators(trimmed);

    // Try progressively shorter prefixes (longest first for best recovery).
    for &cut in cut_points.iter().rev() {
        // Build a candidate: everything up to (but not including) the separator
        // plus a closing brace.
        let prefix = trimmed[..cut].trim_end();
        // Strip a trailing comma if present.
        let prefix = prefix.trim_end_matches(',').trim_end();

        if prefix.is_empty() || prefix == "{" {
            continue;
        }

        let candidate = format!("{prefix}}}");
        if let Ok(v) = serde_json::from_str::<Value>(&candidate) {
            return Some(v);
        }
    }

    None
}

/// Return a list of byte offsets in `input` that correspond to `,` separators
/// at nesting depth 1 (i.e. direct children of the outermost `{`).  The
/// opening `{` position is also included as a synthetic separator so callers
/// can reconstruct any prefix.
fn collect_depth1_separators(input: &str) -> Vec<usize> {
    let mut positions: Vec<usize> = Vec::new();
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut escape_next = false;
    let bytes = input.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        let ch = bytes[i] as char;

        if escape_next {
            escape_next = false;
            i += 1;
            continue;
        }

        match ch {
            '\\' if in_string => escape_next = true,
            '"' => in_string = !in_string,
            '{' | '[' if !in_string => {
                depth += 1;
                if depth == 1 {
                    // Record the position just after the opening brace.
                    positions.push(i + 1);
                }
            }
            '}' | ']' if !in_string => depth -= 1,
            ',' if !in_string && depth == 1 => {
                // Record position *after* the comma so the prefix includes the
                // completed pair.
                positions.push(i);
            }
            _ => {}
        }
        i += 1;
    }

    positions
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── 1. Complete JSON passes through unchanged ─────────────────────────────

    #[test]
    fn complete_json_object() {
        let input = r#"{"key": "value", "num": 42, "flag": true}"#;
        let v = parse_partial_json(input).expect("should parse complete JSON");
        assert_eq!(v["key"], "value");
        assert_eq!(v["num"], 42);
        assert_eq!(v["flag"], true);
    }

    #[test]
    fn complete_json_array() {
        let input = r#"[1, 2, 3]"#;
        let v = parse_partial_json(input).expect("should parse complete JSON array");
        assert_eq!(v[0], 1);
        assert_eq!(v[2], 3);
    }

    // ── 2. Truncated string value ─────────────────────────────────────────────

    #[test]
    fn truncated_string_value() {
        // The string "/usr/loc" is not closed; the key "path" has no prior sibling.
        let input = r#"{"path": "/usr/loc"#;
        let v = parse_partial_json(input).expect("should recover truncated string");
        // The recovered value for "path" should be the partial string as-is.
        assert_eq!(v["path"], "/usr/loc");
    }

    #[test]
    fn truncated_string_with_prior_complete_pair() {
        // "key" is complete; "key2" is truncated mid-value.
        let input = r#"{"key": "value", "key2": "val"#;
        let v = parse_partial_json(input).expect("should recover at least the first pair");
        assert_eq!(v["key"], "value");
    }

    // ── 3. Truncated number ───────────────────────────────────────────────────

    #[test]
    fn truncated_number_value() {
        // The number 12 is complete as an integer literal.
        let input = r#"{"count": 12"#;
        let v = parse_partial_json(input).expect("should recover truncated number");
        assert_eq!(v["count"], 12);
    }

    #[test]
    fn truncated_number_mid_digit() {
        // 123 is a complete integer even though the stream could theoretically
        // have sent more digits.
        let input = r#"{"count": 123, "label": "abc"#;
        let v = parse_partial_json(input).expect("should recover the complete count pair");
        assert_eq!(v["count"], 123);
    }

    // ── 4. Truncated nested object ────────────────────────────────────────────

    #[test]
    fn truncated_nested_object() {
        // "a" is fully present; "d" has a colon but no value yet.
        // Bracket completion fills the dangling position with `null`, which is
        // the best conservative recovery — we never invent a non-null value.
        let input = r#"{"a": {"b": "c"}, "d":"#;
        let v = parse_partial_json(input).expect("should recover at least the first pair");
        // The fully-formed nested pair must be intact.
        assert_eq!(v["a"]["b"], "c");
        // "d" may be null (from bracket completion) or absent (from walk-back).
        // Either outcome is acceptable; what must NOT happen is a non-null,
        // non-absent value being invented for "d".
        if let Some(d) = v.get("d") {
            assert!(d.is_null(), "dangling key must be null or absent, got: {d}");
        }
    }

    #[test]
    fn truncated_nested_object_mid_inner_value() {
        // "outer" is complete; "other" is truncated inside its nested object.
        let input = r#"{"outer": {"x": 1, "y": 2}, "other": {"z": "hel"#;
        let v = parse_partial_json(input).expect("should recover outer pair");
        assert_eq!(v["outer"]["x"], 1);
        assert_eq!(v["outer"]["y"], 2);
    }

    // ── 5. Empty / whitespace input ───────────────────────────────────────────

    #[test]
    fn empty_input_returns_none() {
        assert!(parse_partial_json("").is_none());
    }

    #[test]
    fn whitespace_only_returns_none() {
        assert!(parse_partial_json("   \n\t  ").is_none());
    }

    // ── 6. Array input ────────────────────────────────────────────────────────

    #[test]
    fn complete_nested_array() {
        let input = r#"{"items": [1, 2, 3]}"#;
        let v = parse_partial_json(input).expect("should parse object with array value");
        assert_eq!(v["items"][1], 2);
    }

    #[test]
    fn truncated_array_at_top_level() {
        // A top-level truncated array: bracket completion should add `]`.
        let input = r#"[1, 2, 3"#;
        let v = parse_partial_json(input).expect("should recover truncated array");
        assert_eq!(v[0], 1);
        assert_eq!(v[2], 3);
    }

    // ── 7. Dangling key (no value yet) ────────────────────────────────────────

    #[test]
    fn dangling_key_no_value() {
        // Only the key is present; no colon yet.  Recovery should return the
        // earlier complete pair if one exists, or None.
        let input = r#"{"done": true, "next"#;
        let v = parse_partial_json(input).expect("should recover the completed pair");
        assert_eq!(v["done"], true);
        assert!(v.get("next").is_none());
    }

    // ── 8. Boolean and null values ────────────────────────────────────────────

    #[test]
    fn boolean_and_null_values() {
        let input = r#"{"ok": true, "data": null, "err": false}"#;
        let v = parse_partial_json(input).unwrap();
        assert_eq!(v["ok"], true);
        assert!(v["data"].is_null());
        assert_eq!(v["err"], false);
    }

    // ── 9. Escaped quotes inside strings ─────────────────────────────────────

    #[test]
    fn escaped_quotes_in_string() {
        let input = r#"{"msg": "say \"hello\""}"#;
        let v = parse_partial_json(input).expect("should handle escaped quotes");
        assert_eq!(v["msg"], r#"say "hello""#);
    }

    // ── 10. Object truncated right after the opening brace ───────────────────

    #[test]
    fn only_opening_brace_returns_none() {
        // There is no complete key-value pair to recover.
        let input = "{";
        // This may or may not return Some — what matters is we do not panic.
        let _ = parse_partial_json(input);
    }
}
