use serde::{Deserialize, Serialize};

// ─── Tool definition ──────────────────────────────────────────────────────────

/// Describes a callable tool that can be passed to an LLM provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    /// JSON Schema object describing the tool's parameters.
    pub parameters: serde_json::Value,
}

impl ToolDefinition {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
    ) -> Self {
        ToolDefinition {
            name: name.into(),
            description: description.into(),
            parameters,
        }
    }

    /// Convenience: build a simple no-parameter tool.
    pub fn no_params(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self::new(
            name,
            description,
            serde_json::json!({ "type": "object", "properties": {}, "required": [] }),
        )
    }
}

// ─── Tool call (runtime) ──────────────────────────────────────────────────────

/// A concrete invocation of a tool requested by the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

impl ToolCall {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: serde_json::Value,
    ) -> Self {
        ToolCall { id: id.into(), name: name.into(), arguments }
    }

    /// Parse the arguments JSON if stored as a string (some providers stream
    /// arguments as a raw JSON string rather than a parsed object).
    pub fn parse_arguments(&self) -> crate::error::Result<serde_json::Value> {
        if let serde_json::Value::String(s) = &self.arguments {
            Ok(serde_json::from_str(s)?)
        } else {
            Ok(self.arguments.clone())
        }
    }
}

impl From<&crate::messages::types::Content> for Option<ToolCall> {
    fn from(content: &crate::messages::types::Content) -> Self {
        match content {
            crate::messages::types::Content::ToolCall { id, name, arguments, .. } => {
                Some(ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    arguments: arguments.clone(),
                })
            }
            _ => None,
        }
    }
}

// ─── Tool result ──────────────────────────────────────────────────────────────

/// The outcome of executing a tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_call_id: String,
    pub tool_name: String,
    pub output: serde_json::Value,
    pub is_error: bool,
}

impl ToolResult {
    pub fn success(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        output: serde_json::Value,
    ) -> Self {
        ToolResult {
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            output,
            is_error: false,
        }
    }

    pub fn error(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        ToolResult {
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            output: serde_json::json!({ "error": message.into() }),
            is_error: true,
        }
    }

    pub fn to_message(&self) -> crate::messages::types::Message {
        use crate::messages::types::{Content, Message, ToolResultMessage};
        use chrono::Utc;

        Message::ToolResult(ToolResultMessage {
            tool_call_id: self.tool_call_id.clone(),
            tool_name: self.tool_name.clone(),
            content: vec![Content::Text {
                text: self.output.to_string(),
                text_signature: None,
            }],
            details: None,
            is_error: self.is_error,
            timestamp: Utc::now().timestamp_millis(),
        })
    }
}
