use serde::{Serialize, Deserialize};

/// Result of a compaction operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionResult {
    /// The generated summary replacing old messages
    pub summary: String,
    /// Number of messages that were compacted
    pub messages_compacted: usize,
    /// Token count before compaction
    pub tokens_before: u64,
    /// Token count after compaction
    pub tokens_after: u64,
    /// ID of the first message that was kept (not compacted)
    pub first_kept_id: Option<String>,
}

/// Settings for compaction behavior
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionSettings {
    /// Whether auto-compaction is enabled
    pub enabled: bool,
    /// Token reserve before triggering compaction
    pub reserve_tokens: u64,
    /// Recent tokens to always keep (not compact)
    pub keep_recent_tokens: u64,
}

impl Default for CompactionSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            reserve_tokens: 16384,
            keep_recent_tokens: 20000,
        }
    }
}

/// Estimate token count for a string (chars/4 heuristic, same as pi-mono TS)
pub fn estimate_tokens_str(text: &str) -> u64 {
    (text.len() as u64) / 4
}

/// Find the split point for compaction: compact everything before this index,
/// keep everything from this index onward.
/// Ensures we keep at least `keep_recent_tokens` tokens of recent messages.
pub fn find_compaction_split(
    message_tokens: &[u64],
    keep_recent_tokens: u64,
) -> usize {
    if message_tokens.is_empty() {
        return 0;
    }

    let mut recent_sum: u64 = 0;
    for (i, &tokens) in message_tokens.iter().enumerate().rev() {
        recent_sum += tokens;
        if recent_sum >= keep_recent_tokens {
            return i;
        }
    }

    // If all messages together are less than keep_recent_tokens, don't compact
    0
}

/// Build a compaction prompt that asks the LLM to summarize the conversation
pub fn build_compaction_prompt(messages_text: &str) -> String {
    format!(
        "Summarize the following conversation context concisely. \
         Preserve all important details, decisions, code changes, file paths, \
         and technical context that would be needed to continue the conversation. \
         Be thorough but concise.\n\n\
         ---\n\
         {messages_text}\n\
         ---\n\n\
         Summary:"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_compaction_split() {
        // 5 messages with 1000 tokens each
        let tokens = vec![1000, 1000, 1000, 1000, 1000];

        // Keep 2500 recent tokens -> split at index 2 (keep messages 2,3,4)
        assert_eq!(find_compaction_split(&tokens, 2500), 2);

        // Keep 5000 -> keep all
        assert_eq!(find_compaction_split(&tokens, 5000), 0);

        // Keep 500 -> split at index 4 (keep only message 4)
        assert_eq!(find_compaction_split(&tokens, 500), 4);
    }

    #[test]
    fn test_estimate_tokens() {
        assert_eq!(estimate_tokens_str("hello world!"), 3); // 12 chars / 4
        assert_eq!(estimate_tokens_str(""), 0);
    }
}
