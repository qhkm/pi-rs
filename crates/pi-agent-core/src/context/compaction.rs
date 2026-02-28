use pi_ai::Message;
use serde::{Deserialize, Serialize};

// ─── Summarization prompts (ported from pi-mono compaction.ts) ───────────────

/// System prompt for branch-point summarization.
///
/// This is used when generating a summary to capture the state of the
/// conversation at a specific branch point so that navigating back to that
/// branch and continuing makes sense without full conversation replay.
pub const BRANCH_SUMMARIZATION_SYSTEM_PROMPT: &str = "\
You are a conversation state summarizer for an AI coding agent. \
Your job is to produce a concise but complete snapshot of where the conversation \
stands at a specific point in time (a branch point), so that a developer can \
return to this exact point later and continue meaningfully.

Rules:
- Focus on what matters for continuation: goal, decisions, code state
- List ALL files that were created or modified, with exact paths
- Preserve exact variable names, function names, and technical identifiers
- Note any pending work or next steps explicitly discussed
- Be precise about the state of the code — not just what was planned, but what was done
- Do NOT add information that wasn't present in the conversation
- Use the exact structured format requested";

/// User-side prompt for branch-point summarization.
///
/// The placeholder `{conversation}` is replaced with the serialized messages
/// up to the branch point before the prompt is sent to the LLM.
pub const BRANCH_SUMMARIZATION_PROMPT: &str = "\
Summarize the conversation up to this branch point into a structured snapshot. \
This snapshot must be complete enough for someone to return to this exact point \
and continue working without needing the full conversation history.

Use this exact format:

## Goal
[What the user was trying to accomplish at this branch point]

## Decisions Made
[Key decisions made up to this point, including rationale where given]

## Code / Project State
[Exact state of the code or project — which features/functions were implemented, \
which are partial, which are broken. Be specific about what works and what doesn't.]

## Files Modified
[Exact file paths that were created or modified, one per line, prefixed with the action:
  - created: path/to/file
  - modified: path/to/file
  - deleted: path/to/file]

## Pending / Next Steps
[Work that was explicitly planned or in-flight but not yet complete at this branch point]

## Critical Context
[Error messages, environment details, configuration values, constraints, or anything \
else essential for understanding the state at this branch point]

<conversation>
{conversation}
</conversation>";

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

/// Configuration for branch-point summarization.
///
/// Controls whether branch summaries are generated when the user navigates to
/// a branch point, and how many tokens to budget for the generated summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchSummarizationSettings {
    /// Whether branch-point summarization is enabled.  When `false`,
    /// `summarize_branch_point` returns the raw serialized messages instead
    /// of triggering an LLM call.
    pub enabled: bool,
    /// Token budget reserved for the generated summary.  Corresponds to the
    /// `max_tokens` value passed to the summarization LLM call.
    /// Default: 4096.
    pub reserve_tokens: u64,
}

impl Default for BranchSummarizationSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            reserve_tokens: 4096,
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
                            lines.push(format!("[Assistant] tool_call: {name}({args_display})"));
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

/// Build a prompt pair `(system_prompt, user_prompt)` to summarize the
/// conversation state at a branch point.
///
/// The returned prompts are ready to be forwarded to an LLM — the caller is
/// responsible for actually performing the API call.  Tests should exercise
/// this function directly without making any LLM call.
///
/// # Parameters
/// - `messages_text`: the serialized conversation up to the branch point,
///   typically produced by [`serialize_conversation`].
///
/// # Returns
/// `(system_prompt, user_prompt)` where both strings can be passed to the LLM
/// as the system and user roles respectively.
pub fn build_branch_summary_prompt(messages_text: &str) -> (String, String) {
    let system_prompt = BRANCH_SUMMARIZATION_SYSTEM_PROMPT.to_string();
    let user_prompt = BRANCH_SUMMARIZATION_PROMPT.replace("{conversation}", messages_text);
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
        let (sys, user) = build_compaction_prompt("new conversation", Some("old summary here"));
        assert!(sys.contains("conversation summarizer"));
        assert!(user.contains("<previous-summary>"));
        assert!(user.contains("old summary here"));
        assert!(user.contains("</previous-summary>"));
        assert!(user.contains("<conversation>"));
        assert!(user.contains("new conversation"));
    }

    // ─── Branch summarization tests ──────────────────────────────────────────

    /// `build_branch_summary_prompt` must embed the conversation text and use
    /// the branch-specific system prompt (not the generic compaction one).
    #[test]
    fn test_build_branch_summary_prompt_embeds_conversation() {
        let conversation = "[User]: Please refactor the auth module\n\
                            [Assistant]: I'll start by reading auth.rs\n\
                            [Assistant] tool_call: read_file({\"path\":\"src/auth.rs\"})";

        let (sys, user) = build_branch_summary_prompt(conversation);

        // System prompt must be the branch-specific one, not the generic compaction prompt.
        assert!(
            sys.contains("branch point"),
            "system prompt should mention 'branch point'"
        );
        assert!(
            sys.contains("coding agent"),
            "system prompt should reference coding agent context"
        );
        // Must NOT be the generic compaction system prompt.
        assert!(
            !sys.contains("conversation summarizer"),
            "should not use generic compaction system prompt"
        );

        // User prompt must contain the conversation wrapped in <conversation> tags.
        assert!(
            user.contains("<conversation>"),
            "user prompt must open <conversation> tag"
        );
        assert!(
            user.contains("</conversation>"),
            "user prompt must close </conversation> tag"
        );
        assert!(
            user.contains(conversation),
            "user prompt must embed the full conversation text"
        );

        // User prompt must request all required sections.
        assert!(user.contains("## Goal"), "must request ## Goal section");
        assert!(
            user.contains("## Decisions Made"),
            "must request ## Decisions Made section"
        );
        assert!(
            user.contains("## Code / Project State"),
            "must request ## Code / Project State section"
        );
        assert!(
            user.contains("## Files Modified"),
            "must request ## Files Modified section"
        );
        assert!(
            user.contains("## Pending / Next Steps"),
            "must request ## Pending / Next Steps section"
        );
        assert!(
            user.contains("## Critical Context"),
            "must request ## Critical Context section"
        );
    }

    /// Empty conversation text produces well-formed prompts with empty
    /// `<conversation></conversation>` markers — the LLM can handle this
    /// gracefully.
    #[test]
    fn test_build_branch_summary_prompt_empty_conversation() {
        let (sys, user) = build_branch_summary_prompt("");

        assert!(!sys.is_empty(), "system prompt must not be empty");
        assert!(user.contains("<conversation>"), "must have opening tag");
        assert!(user.contains("</conversation>"), "must have closing tag");
        // The {conversation} placeholder must have been replaced (not left as literal).
        assert!(
            !user.contains("{conversation}"),
            "placeholder must be substituted"
        );
    }

    /// `BranchSummarizationSettings::default()` should produce sane defaults.
    #[test]
    fn test_branch_summarization_settings_defaults() {
        let settings = BranchSummarizationSettings::default();
        assert!(settings.enabled, "should be enabled by default");
        assert_eq!(
            settings.reserve_tokens, 4096,
            "default reserve_tokens should be 4096"
        );
    }

    /// Verify that the branch summary prompt does NOT include a
    /// `<previous-summary>` block (that belongs to incremental compaction, not
    /// branch summaries).
    #[test]
    fn test_build_branch_summary_prompt_has_no_previous_summary_block() {
        let (_, user) = build_branch_summary_prompt("some messages here");
        assert!(
            !user.contains("<previous-summary>"),
            "branch summary prompt must not include a previous-summary block"
        );
        assert!(
            !user.contains("</previous-summary>"),
            "branch summary prompt must not include a previous-summary close tag"
        );
    }
}
