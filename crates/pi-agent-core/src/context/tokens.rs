/// Token estimation utilities for context management.
///
/// Provides functions to estimate token counts for:
/// - Tool definitions (schemas)
/// - Messages
/// - System prompts
use serde_json::Value;

/// Estimate tokens for a tool definition based on its JSON schema.
///
/// Uses a heuristic based on the serialized JSON size:
/// - Base cost: 20 tokens per tool (for boilerplate)
/// - Additional: 1 token per ~4 characters of schema JSON
///
/// This is a rough approximation. For accurate counts, use the provider's
/// tokenizer (e.g., tiktoken for OpenAI, Claude tokenizer for Anthropic).
pub fn estimate_tool_tokens(schema: &Value) -> u64 {
    let base_cost = 20u64;
    
    // Serialize and count characters
    let json_str = match serde_json::to_string(schema) {
        Ok(s) => s,
        Err(_) => return base_cost, // Fallback
    };
    
    // Characters / 4 heuristic, with a minimum for small schemas
    let content_tokens = (json_str.len() as u64).saturating_div(4);
    
    base_cost + content_tokens
}

/// Estimate tokens for multiple tool definitions.
pub fn estimate_tools_tokens(tools: &[pi_ai::ToolDefinition]) -> u64 {
    tools.iter().map(|t| estimate_tool_tokens(&t.parameters)).sum()
}

/// Estimate tokens for a system prompt.
pub fn estimate_system_tokens(system_prompt: &str) -> u64 {
    (system_prompt.len() as u64).saturating_div(4)
}

/// Calculate total context tokens including all components.
pub struct ContextTokenBreakdown {
    pub system_tokens: u64,
    pub message_tokens: u64,
    pub tool_tokens: u64,
    pub total_tokens: u64,
}

/// Calculate complete token breakdown for context.
pub fn calculate_context_tokens(
    messages: &[crate::messages::AgentMessage],
    tools: &[pi_ai::ToolDefinition],
    system_prompt: Option<&str>,
) -> ContextTokenBreakdown {
    let system_tokens = system_prompt.map(estimate_system_tokens).unwrap_or(0);
    let message_tokens: u64 = messages.iter().map(crate::messages::estimate_tokens).sum();
    let tool_tokens = estimate_tools_tokens(tools);
    
    ContextTokenBreakdown {
        system_tokens,
        message_tokens,
        tool_tokens,
        total_tokens: system_tokens + message_tokens + tool_tokens,
    }
}

/// Estimate tokens for a JSON value (for tool arguments/results).
pub fn estimate_json_tokens(value: &Value) -> u64 {
    let json_str = match serde_json::to_string(value) {
        Ok(s) => s,
        Err(_) => return 0,
    };
    (json_str.len() as u64).saturating_div(4)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_estimate_tool_tokens_base() {
        let schema = json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" }
            }
        });
        
        let tokens = estimate_tool_tokens(&schema);
        // Should be at least base cost
        assert!(tokens >= 20, "Tool should have base cost of at least 20 tokens");
    }

    #[test]
    fn test_estimate_tool_tokens_scales_with_size() {
        let small_schema = json!({"type": "object"});
        let large_schema = json!({
            "type": "object",
            "properties": {
                "a": { "type": "string", "description": "A long description here" },
                "b": { "type": "number", "description": "Another description" },
                "c": { "type": "boolean" },
                "d": { "type": "array", "items": { "type": "string" } }
            },
            "required": ["a", "b"]
        });
        
        let small_tokens = estimate_tool_tokens(&small_schema);
        let large_tokens = estimate_tool_tokens(&large_schema);
        
        assert!(large_tokens > small_tokens, "Larger schema should have more tokens");
    }

    #[test]
    fn test_estimate_tools_tokens_multiple() {
        let tools = vec![
            pi_ai::ToolDefinition {
                name: "tool1".to_string(),
                description: "First tool".to_string(),
                parameters: json!({"type": "object"}),
            },
            pi_ai::ToolDefinition {
                name: "tool2".to_string(),
                description: "Second tool".to_string(),
                parameters: json!({"type": "object"}),
            },
        ];
        
        let total = estimate_tools_tokens(&tools);
        let single = estimate_tool_tokens(&json!({"type": "object"}));
        
        assert_eq!(total, single * 2, "Multiple tools should sum their tokens");
    }

    #[test]
    fn test_estimate_system_tokens() {
        let prompt = "You are a helpful assistant.";
        let tokens = estimate_system_tokens(prompt);
        // 30 chars / 4 = ~7 tokens
        assert!(tokens > 0, "System prompt should have tokens");
        assert_eq!(tokens, 30u64 / 4);
    }

    #[test]
    fn test_calculate_context_tokens() {
        use crate::messages::AgentMessage;
        
        let messages = vec![
            AgentMessage::from_llm(pi_ai::Message::user("Hello")),
        ];
        let tools = vec![pi_ai::ToolDefinition {
            name: "test".to_string(),
            description: "Test tool".to_string(),
            parameters: json!({"type": "object"}),
        }];
        
        let breakdown = calculate_context_tokens(&messages, &tools, Some("System prompt"));
        
        assert!(breakdown.system_tokens > 0, "Should have system tokens");
        assert!(breakdown.message_tokens > 0, "Should have message tokens");
        assert!(breakdown.tool_tokens > 0, "Should have tool tokens");
        assert_eq!(
            breakdown.total_tokens,
            breakdown.system_tokens + breakdown.message_tokens + breakdown.tool_tokens,
            "Total should be sum of components"
        );
    }
}
