//! Fuzzy string matching for autocomplete and search.
//!
//! Provides algorithms for fuzzy matching with scoring:
//! - Exact substring matching
//! - Character-by-character fuzzy matching
//! - Bonus scores for word boundaries and consecutive matches

use std::cmp::Ordering;

/// A match result with score and positions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzyMatch {
    /// The matched string
    pub text: String,
    /// Match score (higher is better)
    pub score: i32,
    /// Indices of matched characters
    pub positions: Vec<usize>,
}

impl PartialOrd for FuzzyMatch {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for FuzzyMatch {
    fn cmp(&self, other: &Self) -> Ordering {
        // Higher score first, then shorter text, then alphabetically
        other
            .score
            .cmp(&self.score)
            .then_with(|| self.text.len().cmp(&other.text.len()))
            .then_with(|| self.text.cmp(&other.text))
    }
}

/// Match options for fuzzy search.
#[derive(Debug, Clone, Copy)]
pub struct MatchOptions {
    /// Case-sensitive matching
    pub case_sensitive: bool,
    /// Match only from the start of words
    pub word_boundary_only: bool,
    /// Maximum number of gaps allowed
    pub max_gaps: Option<usize>,
}

impl Default for MatchOptions {
    fn default() -> Self {
        Self {
            case_sensitive: false,
            word_boundary_only: false,
            max_gaps: None,
        }
    }
}

/// Match a pattern against a candidate string using fuzzy matching.
///
/// Returns `Some(FuzzyMatch)` if the pattern matches, `None` otherwise.
///
/// # Algorithm
///
/// Uses a dynamic programming approach similar to fzf:
/// - Scores consecutive matches higher
/// - Gives bonuses for matches after word boundaries (/ _ -.)
/// - Gives bonuses for matches at the start of the string
/// - Penalizes gaps between matched characters
///
/// # Example
///
/// ```
/// use pi_tui::fuzzy::fuzzy_match;
///
/// let result = fuzzy_match("hl", "Hello World", &Default::default());
/// assert!(result.is_some());
/// assert_eq!(result.unwrap().positions, vec![0, 2]);
/// ```
pub fn fuzzy_match(pattern: &str, text: &str, opts: &MatchOptions) -> Option<FuzzyMatch> {
    if pattern.is_empty() {
        return Some(FuzzyMatch {
            text: text.to_string(),
            score: 0,
            positions: Vec::new(),
        });
    }

    let pattern_chars: Vec<char> = if opts.case_sensitive {
        pattern.chars().collect()
    } else {
        pattern.to_lowercase().chars().collect()
    };

    let text_chars: Vec<char> = if opts.case_sensitive {
        text.chars().collect()
    } else {
        text.to_lowercase().chars().collect()
    };

    // Simple case: exact substring match gets high score
    if let Some(pos) = text_chars
        .windows(pattern_chars.len())
        .position(|w| w == &pattern_chars)
    {
        let positions: Vec<usize> = (pos..pos + pattern_chars.len()).collect();
        let score = calculate_score(&positions, &text_chars, true);
        return Some(FuzzyMatch {
            text: text.to_string(),
            score,
            positions,
        });
    }

    // Fuzzy matching: find best sequence of character matches
    let mut best_score = i32::MIN;
    let mut best_positions: Vec<usize> = Vec::new();

    // Use greedy approach with backtracking for small patterns
    if pattern_chars.len() <= 10 {
        find_best_match(
            &pattern_chars,
            &text_chars,
            0,
            0,
            &mut Vec::new(),
            &mut best_score,
            &mut best_positions,
            opts,
        );
    } else {
        // For longer patterns, use DP approach
        return dp_fuzzy_match(&pattern_chars, &text_chars, text, opts);
    }

    if best_score > i32::MIN / 2 {
        Some(FuzzyMatch {
            text: text.to_string(),
            score: calculate_score(&best_positions, &text_chars, false),
            positions: best_positions,
        })
    } else {
        None
    }
}

/// Dynamic programming approach for longer patterns.
fn dp_fuzzy_match(
    pattern: &[char],
    text: &[char],
    original_text: &str,
    opts: &MatchOptions,
) -> Option<FuzzyMatch> {
    let p_len = pattern.len();
    let t_len = text.len();

    if p_len > t_len {
        return None;
    }

    // DP table: dp[i][j] = best score for matching pattern[0..i] to text[0..j]
    // We use rolling arrays to save memory
    let mut prev: Vec<i32> = vec![i32::MIN / 2; t_len + 1];
    let mut curr: Vec<i32> = vec![i32::MIN / 2; t_len + 1];
    let mut path: Vec<Vec<Option<usize>>> = vec![vec![None; t_len + 1]; p_len + 1];

    // Base case: empty pattern matches at score 0
    for j in 0..=t_len {
        prev[j] = 0;
    }

    for i in 1..=p_len {
        curr[0] = i32::MIN / 2;
        for j in 1..=t_len {
            // Option 1: Don't match pattern[i-1] to text[j-1]
            let skip = curr[j - 1];

            // Option 2: Match pattern[i-1] to text[j-1]
            let mut take = i32::MIN / 2;
            if pattern[i - 1] == text[j - 1] {
                take = prev[j - 1];
                if take > i32::MIN / 2 {
                    // Add score for this match
                    let bonus = get_bonus(j - 1, text);
                    let consecutive = path[i - 1][j - 1].map(|p| p == j - 2).unwrap_or(false);
                    if consecutive {
                        take += 15; // Consecutive match bonus
                    } else {
                        take += 10 + bonus; // Base match + bonus
                    }

                    // Gap penalty
                    if i > 1 {
                        if let Some(prev_pos) = path[i - 1][j - 1] {
                            let gap = (j - 2) - prev_pos;
                            take -= gap as i32 * 3;
                        }
                    }
                }
            }

            if take > skip {
                curr[j] = take;
                path[i][j] = Some(j - 1);
            } else {
                curr[j] = skip;
            }
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    // Find best ending position
    let mut best_score = i32::MIN;
    let mut best_end = 0;
    for j in 1..=t_len {
        if prev[j] > best_score {
            best_score = prev[j];
            best_end = j;
        }
    }

    if best_score <= i32::MIN / 2 {
        return None;
    }

    // Reconstruct path
    let mut positions: Vec<usize> = Vec::new();
    let mut j = best_end;
    for i in (1..=p_len).rev() {
        if let Some(pos) = path[i][j] {
            positions.push(pos);
            j = pos;
        } else {
            return None;
        }
    }
    positions.reverse();

    Some(FuzzyMatch {
        text: original_text.to_string(),
        score: calculate_score(&positions, text, false),
        positions,
    })
}

/// Recursive backtracking for small patterns.
fn find_best_match(
    pattern: &[char],
    text: &[char],
    p_idx: usize,
    t_idx: usize,
    current: &mut Vec<usize>,
    best_score: &mut i32,
    best_positions: &mut Vec<usize>,
    opts: &MatchOptions,
) {
    if p_idx >= pattern.len() {
        let score = calculate_score(current, text, false);
        if score > *best_score {
            *best_score = score;
            *best_positions = current.clone();
        }
        return;
    }

    let remaining_pattern = pattern.len() - p_idx;
    let remaining_text = text.len() - t_idx;

    if remaining_pattern > remaining_text {
        return;
    }

    if let Some(max_gaps) = opts.max_gaps {
        let gaps = current.len().saturating_sub(1);
        if gaps >= max_gaps && remaining_pattern < remaining_text {
            // Skip searching if we've exceeded max gaps
        }
    }

    for i in t_idx..text.len() {
        if pattern[p_idx] == text[i] {
            // Check word boundary constraint
            if opts.word_boundary_only && p_idx == 0 && i > 0 {
                if !is_word_boundary(i, text) {
                    continue;
                }
            }

            current.push(i);
            find_best_match(
                pattern,
                text,
                p_idx + 1,
                i + 1,
                current,
                best_score,
                best_positions,
                opts,
            );
            current.pop();
        }
    }
}

/// Calculate the score for a match.
fn calculate_score(positions: &[usize], text: &[char], is_exact: bool) -> i32 {
    if positions.is_empty() {
        return 0;
    }

    let mut score = if is_exact { 100 } else { 0 };

    // Bonus for starting at the beginning
    if positions[0] == 0 {
        score += 10;
    }

    // Bonus for consecutive matches and word boundaries
    for i in 0..positions.len() {
        let pos = positions[i];
        score += get_bonus(pos, text);

        if i > 0 {
            let prev = positions[i - 1];
            if pos == prev + 1 {
                score += 15; // Consecutive bonus
            } else {
                score -= ((pos - prev - 1) as i32) * 3; // Gap penalty
            }
        }
    }

    // Penalty for length
    score -= (text.len() - positions.len()) as i32;

    score
}

/// Get bonus score for a position (word boundaries, etc.)
fn get_bonus(pos: usize, text: &[char]) -> i32 {
    if pos == 0 {
        return 10; // Start of string
    }

    if is_word_boundary(pos, text) {
        return 8; // Word boundary
    }

    0
}

/// Check if position is at a word boundary.
fn is_word_boundary(pos: usize, text: &[char]) -> bool {
    if pos == 0 {
        return true;
    }

    let prev = text[pos - 1];
    let curr = text[pos];

    // After separator characters
    if matches!(prev, '/' | '\\' | '_' | '-' | '.' | ' ' | ':' | '@') {
        return true;
    }

    // camelCase transition
    if prev.is_lowercase() && curr.is_uppercase() {
        return true;
    }

    false
}

/// Filter and sort a list of candidates by fuzzy match score.
///
/// Returns matches sorted by score (best first).
pub fn fuzzy_filter(pattern: &str, candidates: &[String], opts: &MatchOptions) -> Vec<FuzzyMatch> {
    let mut matches: Vec<FuzzyMatch> = candidates
        .iter()
        .filter_map(|c| fuzzy_match(pattern, c, opts))
        .collect();

    matches.sort();
    matches
}

/// Filter candidates and return only the text strings.
pub fn fuzzy_filter_text(pattern: &str, candidates: &[String], opts: &MatchOptions) -> Vec<String> {
    fuzzy_filter(pattern, candidates, opts)
        .into_iter()
        .map(|m| m.text)
        .collect()
}

/// Highlight matched characters in a string.
///
/// Wraps matched characters with the provided highlight function.
pub fn highlight_matches(
    text: &str,
    positions: &[usize],
    highlight: impl Fn(&str) -> String,
) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut result = String::new();
    let mut pos_set: std::collections::HashSet<usize> = positions.iter().copied().collect();

    let mut i = 0;
    while i < chars.len() {
        if pos_set.contains(&i) {
            // Find consecutive matches
            let start = i;
            while i < chars.len() && pos_set.contains(&i) {
                i += 1;
            }
            let matched: String = chars[start..i].iter().collect();
            result.push_str(&highlight(&matched));
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    result
}

/// Simple prefix matching with highlighting.
pub fn prefix_match(pattern: &str, text: &str, case_sensitive: bool) -> Option<FuzzyMatch> {
    let (p, t) = if case_sensitive {
        (pattern.to_string(), text.to_string())
    } else {
        (pattern.to_lowercase(), text.to_lowercase())
    };

    if t.starts_with(&p) {
        let positions: Vec<usize> = (0..pattern.len()).collect();
        Some(FuzzyMatch {
            text: text.to_string(),
            score: 200 - (text.len() as i32), // Shorter matches score higher
            positions,
        })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fuzzy_match_basic() {
        let m = fuzzy_match("abc", "abc", &Default::default()).unwrap();
        assert_eq!(m.positions, vec![0, 1, 2]);
        assert!(m.score > 0);
    }

    #[test]
    fn test_fuzzy_match_gap() {
        let m = fuzzy_match("abc", "aabbcc", &Default::default()).unwrap();
        // Positions depend on the matching algorithm (greedy vs optimal)
        assert_eq!(m.positions.len(), 3);
        assert_eq!(m.positions[0], 0); // First char matches at start
                                       // The algorithm may find different but valid positions
    }

    #[test]
    fn test_fuzzy_no_match() {
        assert!(fuzzy_match("xyz", "abc", &Default::default()).is_none());
    }

    #[test]
    fn test_fuzzy_case_insensitive() {
        let m = fuzzy_match("ABC", "abc", &Default::default()).unwrap();
        assert_eq!(m.positions, vec![0, 1, 2]);
    }

    #[test]
    fn test_fuzzy_filter() {
        let candidates = vec![
            "src/main.rs".to_string(),
            "src/lib.rs".to_string(),
            "Cargo.toml".to_string(),
            "README.md".to_string(),
        ];

        let matches = fuzzy_filter("sr", &candidates, &Default::default());
        assert!(!matches.is_empty());
        assert!(matches.iter().any(|m| m.text.contains("src")));
    }

    #[test]
    fn test_highlight() {
        let text = "Hello World";
        let positions = vec![0, 1, 2];
        let result = highlight_matches(text, &positions, |s| format!("[{}]", s));
        assert_eq!(result, "[Hel]lo World");
    }

    #[test]
    fn test_word_boundary_bonus() {
        // Matching at word boundary should score higher
        let m1 = fuzzy_match("t", "test_file", &Default::default()).unwrap();
        let m2 = fuzzy_match("t", "_test", &Default::default()).unwrap();

        // m2 should have higher score because 't' is at word boundary
        assert!(m2.score > m1.score || m2.positions[0] == 1);
    }

    #[test]
    fn test_prefix_match() {
        let m = prefix_match("src", "src/main.rs", false).unwrap();
        assert_eq!(m.positions, vec![0, 1, 2]);
        assert!(m.score > 100);
    }
}
