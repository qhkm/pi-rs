/// Message transformation utilities for cross-provider compatibility.
///
/// Providers differ in how they handle thinking blocks, tool call IDs, and
/// multi-turn conversations. These transformations normalise a message list
/// before it is sent to any provider.
use std::collections::HashSet;

use crate::messages::types::{
    AssistantMessage, Content, Message, StopReason, ToolResultMessage, UserContent, UserMessage,
};
use chrono::Utc;

// ─── Options ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct TransformOptions {
    /// Strip thinking / reasoning content blocks.
    pub strip_thinking: bool,
    /// Normalise tool call IDs to be simple sequential strings when the
    /// provider does not support arbitrary IDs (e.g. Gemini).
    pub normalise_tool_ids: bool,
    /// Remove assistant messages whose tool calls have no matching tool-result
    /// message that follows them (orphaned tool calls confuse some providers).
    pub remove_orphaned_tool_calls: bool,
    /// Merge consecutive user messages into one (required by Anthropic).
    pub merge_consecutive_user: bool,
    /// Merge consecutive assistant messages into one (required by some providers).
    pub merge_consecutive_assistant: bool,
}

impl TransformOptions {
    pub fn for_anthropic() -> Self {
        TransformOptions {
            strip_thinking: false,
            normalise_tool_ids: false,
            remove_orphaned_tool_calls: true,
            merge_consecutive_user: true,
            merge_consecutive_assistant: true,
        }
    }

    pub fn for_openai() -> Self {
        TransformOptions {
            strip_thinking: true,
            normalise_tool_ids: false,
            remove_orphaned_tool_calls: true,
            merge_consecutive_user: false,
            merge_consecutive_assistant: true,
        }
    }

    pub fn for_google() -> Self {
        TransformOptions {
            strip_thinking: true,
            normalise_tool_ids: true,
            remove_orphaned_tool_calls: true,
            merge_consecutive_user: false,
            merge_consecutive_assistant: true,
        }
    }
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Transform a slice of messages for cross-provider compatibility.
///
/// Returns a new `Vec<Message>` — the input is never mutated.
pub fn transform_messages(messages: &[Message], opts: &TransformOptions) -> Vec<Message> {
    let mut result: Vec<Message> = messages.to_vec();

    if opts.remove_orphaned_tool_calls {
        result = remove_orphaned_tool_calls(result);
    }

    if opts.strip_thinking {
        result = strip_thinking_blocks(result);
    }

    if opts.normalise_tool_ids {
        result = normalise_tool_call_ids(result);
    }

    if opts.merge_consecutive_user {
        result = merge_consecutive_user_messages(result);
    }

    if opts.merge_consecutive_assistant {
        result = merge_consecutive_assistant_messages(result);
    }

    result
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Remove assistant messages that have tool calls without a following
/// ToolResult that references the same IDs.
fn remove_orphaned_tool_calls(messages: Vec<Message>) -> Vec<Message> {
    // Collect every tool_call_id that appears in a ToolResult message.
    let answered_ids: HashSet<String> = messages
        .iter()
        .filter_map(|m| m.as_tool_result())
        .map(|tr| tr.tool_call_id.clone())
        .collect();

    messages
        .into_iter()
        .filter(|m| {
            if let Message::Assistant(am) = m {
                if am.stop_reason == StopReason::ToolUse {
                    // Keep only if every tool call in this message has a result.
                    let all_answered = am.content.iter().filter(|c| c.is_tool_call()).all(|c| {
                        if let Some(id) = c.tool_call_id() {
                            answered_ids.contains(id)
                        } else {
                            false
                        }
                    });
                    return all_answered;
                }
            }
            true
        })
        .collect()
}

/// Strip thinking / reasoning content blocks from assistant messages.
fn strip_thinking_blocks(messages: Vec<Message>) -> Vec<Message> {
    messages
        .into_iter()
        .map(|m| match m {
            Message::Assistant(mut am) => {
                am.content
                    .retain(|c| !matches!(c, Content::Thinking { .. }));
                Message::Assistant(am)
            }
            other => other,
        })
        .collect()
}

/// Normalise tool call IDs to sequential integers ("1", "2", …).  Some
/// providers (Google Gemini) require IDs to be simple strings.
fn normalise_tool_call_ids(messages: Vec<Message>) -> Vec<Message> {
    // Build a deterministic mapping original_id → normalised_id.
    let mut counter = 0u64;
    let mut id_map: std::collections::HashMap<String, String> = std::collections::HashMap::new();

    // First pass: collect all IDs in order.
    for m in &messages {
        match m {
            Message::Assistant(am) => {
                for c in &am.content {
                    if let Content::ToolCall { id, .. } = c {
                        if !id_map.contains_key(id) {
                            counter += 1;
                            id_map.insert(id.clone(), counter.to_string());
                        }
                    }
                }
            }
            Message::ToolResult(tr) => {
                if !id_map.contains_key(&tr.tool_call_id) {
                    counter += 1;
                    id_map.insert(tr.tool_call_id.clone(), counter.to_string());
                }
            }
            _ => {}
        }
    }

    // Second pass: rewrite IDs.
    messages
        .into_iter()
        .map(|m| match m {
            Message::Assistant(mut am) => {
                am.content = am
                    .content
                    .into_iter()
                    .map(|c| match c {
                        Content::ToolCall {
                            id,
                            name,
                            arguments,
                            thought_signature,
                        } => {
                            let new_id = id_map.get(&id).cloned().unwrap_or(id);
                            Content::ToolCall {
                                id: new_id,
                                name,
                                arguments,
                                thought_signature,
                            }
                        }
                        other => other,
                    })
                    .collect();
                Message::Assistant(am)
            }
            Message::ToolResult(mut tr) => {
                tr.tool_call_id = id_map
                    .get(&tr.tool_call_id)
                    .cloned()
                    .unwrap_or(tr.tool_call_id);
                Message::ToolResult(tr)
            }
            other => other,
        })
        .collect()
}

/// Merge consecutive user messages into one, combining their content blocks.
fn merge_consecutive_user_messages(messages: Vec<Message>) -> Vec<Message> {
    let mut result: Vec<Message> = Vec::with_capacity(messages.len());
    for msg in messages {
        match msg {
            Message::User(um) => {
                if let Some(Message::User(prev)) = result.last_mut() {
                    // Merge content into previous user message.
                    let new_blocks = to_blocks(um.content);
                    match &mut prev.content {
                        UserContent::Text(t) => {
                            let mut blocks = vec![Content::Text {
                                text: t.clone(),
                                text_signature: None,
                            }];
                            blocks.extend(new_blocks);
                            prev.content = UserContent::Blocks(blocks);
                        }
                        UserContent::Blocks(b) => {
                            b.extend(new_blocks);
                        }
                    }
                } else {
                    result.push(Message::User(um));
                }
            }
            other => result.push(other),
        }
    }
    result
}

/// Merge consecutive assistant messages into one, combining their content.
fn merge_consecutive_assistant_messages(messages: Vec<Message>) -> Vec<Message> {
    let mut result: Vec<Message> = Vec::with_capacity(messages.len());
    for msg in messages {
        match msg {
            Message::Assistant(am) => {
                if let Some(Message::Assistant(prev)) = result.last_mut() {
                    prev.content.extend(am.content);
                    prev.usage.add(&am.usage);
                    // Keep the latest stop_reason and timestamp.
                    prev.stop_reason = am.stop_reason;
                    prev.timestamp = am.timestamp;
                } else {
                    result.push(Message::Assistant(am));
                }
            }
            other => result.push(other),
        }
    }
    result
}

fn to_blocks(content: UserContent) -> Vec<Content> {
    match content {
        UserContent::Text(t) => vec![Content::Text {
            text: t,
            text_signature: None,
        }],
        UserContent::Blocks(b) => b,
    }
}

// ─── Convenience constructors ─────────────────────────────────────────────────

/// Create a simple text-only user message (convenience re-export).
pub fn user_message(text: impl Into<String>) -> Message {
    Message::User(UserMessage {
        content: UserContent::Text(text.into()),
        timestamp: Utc::now().timestamp_millis(),
    })
}

/// Create a tool-result message (convenience helper).
pub fn tool_result_message(
    tool_call_id: impl Into<String>,
    tool_name: impl Into<String>,
    text: impl Into<String>,
    is_error: bool,
) -> Message {
    Message::ToolResult(ToolResultMessage {
        tool_call_id: tool_call_id.into(),
        tool_name: tool_name.into(),
        content: vec![Content::Text {
            text: text.into(),
            text_signature: None,
        }],
        details: None,
        is_error,
        timestamp: Utc::now().timestamp_millis(),
    })
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::types::{Api, AssistantMessage, Provider, Usage};

    fn make_assistant(content: Vec<Content>, stop_reason: StopReason) -> Message {
        Message::Assistant(AssistantMessage {
            content,
            api: Api::AnthropicMessages,
            provider: Provider::Anthropic,
            model: "test".to_string(),
            usage: Usage::default(),
            stop_reason,
            error_message: None,
            timestamp: 0,
        })
    }

    #[test]
    fn test_strip_thinking() {
        let messages = vec![make_assistant(
            vec![
                Content::Thinking {
                    thinking: "hmm".into(),
                    thinking_signature: None,
                    redacted: false,
                },
                Content::Text {
                    text: "hello".into(),
                    text_signature: None,
                },
            ],
            StopReason::Stop,
        )];

        let opts = TransformOptions {
            strip_thinking: true,
            ..Default::default()
        };
        let result = transform_messages(&messages, &opts);

        assert_eq!(result.len(), 1);
        let am = result[0].as_assistant().unwrap();
        assert_eq!(am.content.len(), 1);
        assert!(matches!(&am.content[0], Content::Text { .. }));
    }

    #[test]
    fn test_merge_consecutive_user() {
        let messages = vec![Message::user("Hello"), Message::user("World")];

        let opts = TransformOptions {
            merge_consecutive_user: true,
            ..Default::default()
        };
        let result = transform_messages(&messages, &opts);

        assert_eq!(result.len(), 1);
        let um = result[0].as_user().unwrap();
        match &um.content {
            UserContent::Blocks(b) => assert_eq!(b.len(), 2),
            _ => panic!("Expected blocks"),
        }
    }
}
