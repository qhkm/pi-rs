use pi_ai::Message;
use serde::{Deserialize, Serialize};

// ─── Summarization prompts (ported from pi-mono compaction.ts) ───────────────

/// System prompt for the summarization LLM call
pub const SUMMARIZATION_SYSTEM_PROMPT: &str = "\
You are a conversation summarizer. Your job is to create a structured summary \
of a conversation between a user and an AI assistant. The summary must preserve \
all critical context needed to continue the conversation seamlessly.

Rules:
- Be thorough but concise
- Preserve ALL file paths, code snippets, variable names, and technical details
- Preserve the user's intent and constraints
- Preserve any decisions made and their rationale
- Do NOT add information that wasn't in the conversation
- Use the exact structured format requested";

/// The structured summary format prompt for first-time compaction
pub const SUMMARIZATION_PROMPT: &str = "\
Summarize the following conversation into a structured format. \
Preserve all important technical context, decisions, and progress.

Use this exact format:

## Goal
[What the user is trying to accomplish]

## Constraints
[Any constraints, preferences, or requirements mentioned]

## Progress
[What has been done so far, including specific files modified, commands run, etc.]

## Key Decisions
[Important decisions made and their rationale]

## Next Steps
[What still needs to be done, if mentioned]

## Critical Context
[Any other important context: error messages, configuration details, environment info, etc.]

<conversation>
{conversation}
</conversation>";

/// Prompt for incremental updates when a previous summary exists
pub const UPDATE_SUMMARIZATION_PROMPT: &str = "\
Update the following summary with new conversation content. \
Merge new information into the existing structure. \
Remove outdated information that has been superseded. \
Preserve all important technical context, decisions, and progress.

Use this exact format:

## Goal
[What the user is trying to accomplish — update if goal has evolved]

## Constraints
[Any constraints, preferences, or requirements mentioned — add new ones]

## Progress
[What has been done so far — add new progress, mark completed items]

## Key Decisions
[Important decisions made and their rationale — add new decisions]

## Next Steps
[What still needs to be done — update based on progress]

## Critical Context
[Any other important context — update with new information]

<previous-summary>
{previous_summary}
</previous-summary>

<conversation>
{conversation}
</conversation>";

// ─── Types ───────────────────────────────────────────────────────────────────

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

// ─── Functions ───────────────────────────────────────────────────────────────

/// Estimate token count for a string (chars/4 heuristic, same as pi-mono TS)
pub fn estimate_tokens_str(text: &str) -> u64 {
    (text.len() as u64) / 4
}

/// Serialize a slice of LLM messages to a human-readable text format.
///
/// Ported from pi-mono `serializeConversation` in utils.ts.
/// Produces lines like:
/// - `[User]: <text>`
/// - `[Assistant]: <text>`
/// - `[Assistant] (thinking): <text>`
/// - `[Assistant] tool_call: <name>(<args>)`
/// - `[Tool result (<name>)]: <text>`
pub fn serialize_conversation(messages: &[Message]) -> String {
    let mut lines: Vec<String> = Vec::new();

    for msg in messages {
        match msg {
            Message::User(_user_msg) => {
                let text = msg.text_content();
                lines.push(format!("[User]: {text}"));
            }
            Message::Assistant(assistant_msg) => {
                for content in &assistant_msg.content {
                    match content {
                        pi_ai::Content::Text { text, .. } => {
                            lines.push(format!("[Assistant]: {text}"));
                        }
                        pi_ai::Content::Thinking { thinking, .. } => {
                            lines.push(format!("[Assistant] (thinking): {thinking}"));
                        }
                        pi_ai::Content::ToolCall {
                            name, arguments, ..
                        } => {
                            let args_str = arguments.to_string();
                            // Truncate very long tool arguments to keep summary manageable
                            let args_display = if args_str.len() > 500 {
                                format!("{}...", &args_str[..500])
                            } else {
                                args_str
                            };
                            lines.push(format!(
                                "[Assistant] tool_call: {name}({args_display})"
                            ));
                        }
                        pi_ai::Content::Image { .. } => {
                            lines.push("[Assistant]: [image]".to_string());
                        }
                    }
                }
            }
            Message::ToolResult(tool_result) => {
                let text = msg.text_content();
                let name = &tool_result.tool_name;
                // Truncate very long tool results
                let display = if text.len() > 1000 {
                    format!("{}...", &text[..1000])
                } else {
                    text
                };
                if tool_result.is_error {
                    lines.push(format!("[Tool result ({name})] ERROR: {display}"));
                } else {
                    lines.push(format!("[Tool result ({name})]: {display}"));
                }
            }
        }
    }

    lines.join("\n")
}

/// Check whether auto-compaction should trigger based on current token usage.
pub fn should_compact(
    context_tokens: u64,
    context_window: u64,
    settings: &CompactionSettings,
) -> bool {
    if !settings.enabled {
        return false;
    }
    context_tokens > context_window.saturating_sub(settings.reserve_tokens)
}

/// Find the split point for compaction: compact everything before this index,
/// keep everything from this index onward.
/// Ensures we keep at least `keep_recent_tokens` tokens of recent messages.
pub fn find_compaction_split(message_tokens: &[u64], keep_recent_tokens: u64) -> usize {
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

/// Build a compaction prompt pair (system_prompt, user_prompt) that asks the
/// LLM to summarize the conversation.
///
/// If `previous_summary` is `Some`, uses the incremental update prompt which
/// includes the previous summary for merging. Otherwise uses the initial
/// summarization prompt.
///
/// Returns `(system_prompt, user_prompt)`.
pub fn build_compaction_prompt(
    messages_text: &str,
    previous_summary: Option<&str>,
) -> (String, String) {
    let system_prompt = SUMMARIZATION_SYSTEM_PROMPT.to_string();

    let user_prompt = match previous_summary {
        Some(prev) => UPDATE_SUMMARIZATION_PROMPT
            .replace("{previous_summary}", prev)
            .replace("{conversation}", messages_text),
        None => SUMMARIZATION_PROMPT.replace("{conversation}", messages_text),
    };

    (system_prompt, user_prompt)
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

    #[test]
    fn test_should_compact() {
        let settings = CompactionSettings {
            enabled: true,
            reserve_tokens: 10000,
            keep_recent_tokens: 5000,
        };

        // context_window = 100000, reserve = 10000 => threshold = 90000
        assert!(!should_compact(80000, 100000, &settings));
        assert!(should_compact(91000, 100000, &settings));
        assert!(should_compact(100000, 100000, &settings));

        // Disabled
        let disabled = CompactionSettings {
            enabled: false,
            ..settings
        };
        assert!(!should_compact(100000, 100000, &disabled));
    }

    #[test]
    fn test_serialize_conversation() {
        let messages = vec![
            Message::user("Hello, can you help me?"),
            // We can't easily construct an AssistantMessage here without
            // all the required fields, so we test user + tool_result
        ];
        let text = serialize_conversation(&messages);
        assert!(text.contains("[User]: Hello, can you help me?"));
    }

    #[test]
    fn test_build_compaction_prompt_initial() {
        let (sys, user) = build_compaction_prompt("some conversation", None);
        assert!(sys.contains("conversation summarizer"));
        assert!(user.contains("<conversation>"));
        assert!(user.contains("some conversation"));
        assert!(user.contains("</conversation>"));
        assert!(!user.contains("<previous-summary>"));
    }

    #[test]
    fn test_build_compaction_prompt_with_previous() {
        let (sys, user) =
            build_compaction_prompt("new conversation", Some("old summary here"));
        assert!(sys.contains("conversation summarizer"));
        assert!(user.contains("<previous-summary>"));
        assert!(user.contains("old summary here"));
        assert!(user.contains("</previous-summary>"));
        assert!(user.contains("<conversation>"));
        assert!(user.contains("new conversation"));
    }
}
