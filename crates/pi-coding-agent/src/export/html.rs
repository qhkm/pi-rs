use std::path::{Path, PathBuf};

use anyhow::Result;
use pi_agent_core::messages::AgentMessage;
use pi_ai::{Content, Message, UserContent};

/// Export a session's messages to a standalone HTML file.
///
/// Returns the path to the generated HTML file.
pub async fn export_to_html(
    messages: &[AgentMessage],
    output_path: Option<&Path>,
    session_name: Option<&str>,
) -> Result<PathBuf> {
    let title = session_name.unwrap_or("Pi Session Export");
    let html = render_html(messages, title);

    let path = output_path.map(PathBuf::from).unwrap_or_else(|| {
        let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
        PathBuf::from(format!("session_export_{}.html", timestamp))
    });

    tokio::fs::write(&path, html).await?;
    Ok(path)
}

fn render_html(messages: &[AgentMessage], title: &str) -> String {
    let mut body = String::new();

    for msg in messages {
        match msg {
            AgentMessage::Llm(message) => match message {
                Message::User(user_msg) => {
                    body.push_str(&format!(
                        "<div class=\"message user\"><div class=\"role\">User</div>\
                         <div class=\"content\">{}</div></div>\n",
                        escape_html(&render_user_content(&user_msg.content))
                    ));
                }
                Message::Assistant(assistant_msg) => {
                    let mut parts = Vec::new();
                    for content in &assistant_msg.content {
                        match content {
                            Content::Text { text, .. } => {
                                parts.push(format!("<p>{}</p>", escape_html(text)));
                            }
                            Content::Thinking { thinking, .. } => {
                                parts.push(format!(
                                    "<details class=\"thinking\"><summary>Thinking</summary>\
                                     <pre>{}</pre></details>",
                                    escape_html(thinking)
                                ));
                            }
                            Content::ToolCall {
                                name, arguments, ..
                            } => {
                                let args_str = serde_json::to_string_pretty(arguments)
                                    .unwrap_or_else(|_| arguments.to_string());
                                parts.push(format!(
                                    "<details class=\"tool-call\"><summary>Tool: {}</summary>\
                                     <pre>{}</pre></details>",
                                    escape_html(name),
                                    escape_html(&args_str)
                                ));
                            }
                            Content::Image { .. } => {
                                parts.push("<p><em>[image]</em></p>".to_string());
                            }
                        }
                    }
                    body.push_str(&format!(
                        "<div class=\"message assistant\"><div class=\"role\">Assistant</div>\
                         <div class=\"content\">{}</div></div>\n",
                        parts.join("\n")
                    ));
                }
                Message::ToolResult(tool_result) => {
                    let content_text: String = tool_result
                        .content
                        .iter()
                        .filter_map(|c| {
                            if let Content::Text { text, .. } = c {
                                Some(text.as_str())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    let truncated = if content_text.len() > 2000 {
                        format!(
                            "{}...\n[truncated {} chars]",
                            &content_text[..2000],
                            content_text.len() - 2000
                        )
                    } else {
                        content_text
                    };
                    body.push_str(&format!(
                        "<div class=\"message tool-result\"><div class=\"role\">Tool: {}</div>\
                         <pre class=\"content\">{}</pre></div>\n",
                        escape_html(&tool_result.tool_name),
                        escape_html(&truncated)
                    ));
                }
            },
            AgentMessage::CompactionSummary { summary, .. } => {
                body.push_str(&format!(
                    "<div class=\"message compaction\"><div class=\"role\">Context Summary</div>\
                     <div class=\"content\"><pre>{}</pre></div></div>\n",
                    escape_html(summary)
                ));
            }
            AgentMessage::SystemContext { content, .. } => {
                body.push_str(&format!(
                    "<div class=\"message system\"><div class=\"role\">System</div>\
                     <div class=\"content\">{}</div></div>\n",
                    escape_html(content)
                ));
            }
            AgentMessage::Extension { .. } => {
                // Extensions are not rendered in the HTML export
            }
        }
    }

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>{title}</title>
<style>
* {{ margin: 0; padding: 0; box-sizing: border-box; }}
body {{ font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; background: #1a1a2e; color: #e0e0e0; padding: 2rem; max-width: 900px; margin: 0 auto; }}
h1 {{ color: #fff; margin-bottom: 2rem; font-size: 1.5rem; }}
.message {{ margin-bottom: 1.5rem; padding: 1rem; border-radius: 8px; border-left: 4px solid #333; }}
.message.user {{ background: #16213e; border-left-color: #0f3460; }}
.message.assistant {{ background: #1a1a2e; border-left-color: #e94560; }}
.message.tool-result {{ background: #0f0f23; border-left-color: #533483; font-size: 0.9rem; }}
.message.compaction {{ background: #1d1d3b; border-left-color: #ffc107; }}
.message.system {{ background: #0d1b2a; border-left-color: #48bfe3; }}
.role {{ font-weight: 600; margin-bottom: 0.5rem; font-size: 0.85rem; text-transform: uppercase; letter-spacing: 0.05em; color: #888; }}
.content {{ line-height: 1.6; white-space: pre-wrap; word-wrap: break-word; }}
pre {{ background: #0d1117; padding: 1rem; border-radius: 6px; overflow-x: auto; font-size: 0.85rem; line-height: 1.4; }}
details {{ margin: 0.5rem 0; }}
summary {{ cursor: pointer; color: #888; font-size: 0.85rem; }}
summary:hover {{ color: #ccc; }}
.thinking pre {{ color: #8b949e; }}
.footer {{ margin-top: 3rem; padding-top: 1rem; border-top: 1px solid #333; color: #666; font-size: 0.8rem; text-align: center; }}
</style>
</head>
<body>
<h1>{title}</h1>
{body}
<div class="footer">Exported from Pi Agent</div>
</body>
</html>"#,
        title = escape_html(title),
        body = body
    )
}

fn render_user_content(content: &UserContent) -> String {
    match content {
        UserContent::Text(text) => text.clone(),
        UserContent::Blocks(blocks) => blocks
            .iter()
            .filter_map(|c| match c {
                Content::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use pi_ai::{Api, AssistantMessage, Provider, StopReason, Usage};

    #[test]
    fn escape_html_encodes_special_chars() {
        assert_eq!(escape_html("<div>"), "&lt;div&gt;");
        assert_eq!(escape_html("a & b"), "a &amp; b");
        assert_eq!(escape_html("x=\"y\""), "x=&quot;y&quot;");
        assert_eq!(escape_html("hello"), "hello");
        assert_eq!(escape_html(""), "");
        assert_eq!(
            escape_html("<script>alert(\"xss\")</script>"),
            "&lt;script&gt;alert(&quot;xss&quot;)&lt;/script&gt;"
        );
    }

    #[test]
    fn render_user_content_text() {
        let content = UserContent::Text("hello world".to_string());
        assert_eq!(render_user_content(&content), "hello world");
    }

    #[test]
    fn render_user_content_blocks() {
        let blocks = vec![
            Content::text("line one"),
            Content::text("line two"),
            Content::image("base64data", "image/png"),
        ];
        let content = UserContent::Blocks(blocks);
        assert_eq!(render_user_content(&content), "line one\nline two");
    }

    fn make_user_agent_message(text: &str) -> AgentMessage {
        AgentMessage::from_llm(Message::user(text))
    }

    fn make_assistant_agent_message(text: &str) -> AgentMessage {
        AgentMessage::from_llm(Message::Assistant(AssistantMessage {
            content: vec![Content::text(text)],
            api: Api::AnthropicMessages,
            provider: Provider::Anthropic,
            model: "test-model".to_string(),
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: Utc::now().timestamp_millis(),
        }))
    }

    #[test]
    fn render_html_produces_valid_structure() {
        let messages = vec![
            make_user_agent_message("Hello"),
            make_assistant_agent_message("Hi there"),
        ];

        let html = render_html(&messages, "Test Session");

        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("<title>Test Session</title>"));
        assert!(html.contains("class=\"message user\""));
        assert!(html.contains("class=\"message assistant\""));
        assert!(html.contains("Hello"));
        assert!(html.contains("Hi there"));
        assert!(html.contains("Exported from Pi Agent"));
    }

    #[test]
    fn render_html_escapes_title() {
        let messages = vec![];
        let html = render_html(&messages, "Test <script> & \"quotes\"");

        assert!(html.contains("Test &lt;script&gt; &amp; &quot;quotes&quot;"));
    }

    #[test]
    fn render_html_includes_tool_calls() {
        let messages = vec![AgentMessage::from_llm(Message::Assistant(
            AssistantMessage {
                content: vec![Content::tool_call(
                    "tc-1",
                    "read_file",
                    serde_json::json!({"path": "/tmp/test.rs"}),
                )],
                api: Api::AnthropicMessages,
                provider: Provider::Anthropic,
                model: "test-model".to_string(),
                usage: Usage::default(),
                stop_reason: StopReason::ToolUse,
                error_message: None,
                timestamp: Utc::now().timestamp_millis(),
            },
        ))];

        let html = render_html(&messages, "Tools");
        assert!(html.contains("Tool: read_file"));
        assert!(html.contains("/tmp/test.rs"));
    }

    #[test]
    fn render_html_includes_compaction_summary() {
        let messages = vec![AgentMessage::CompactionSummary {
            summary: "Previous conversation about Rust types".to_string(),
            tokens_before: 5000,
            timestamp: Utc::now().timestamp_millis(),
        }];

        let html = render_html(&messages, "Compaction");
        assert!(html.contains("class=\"message compaction\""));
        assert!(html.contains("Context Summary"));
        assert!(html.contains("Previous conversation about Rust types"));
    }

    #[test]
    fn render_html_includes_system_context() {
        let messages = vec![AgentMessage::SystemContext {
            content: "You are a helpful assistant.".to_string(),
            source: "system".to_string(),
        }];

        let html = render_html(&messages, "System");
        assert!(html.contains("class=\"message system\""));
        assert!(html.contains("You are a helpful assistant."));
    }

    #[test]
    fn render_html_includes_thinking() {
        let messages = vec![AgentMessage::from_llm(Message::Assistant(
            AssistantMessage {
                content: vec![
                    Content::thinking("Let me think about this..."),
                    Content::text("Here is my answer."),
                ],
                api: Api::AnthropicMessages,
                provider: Provider::Anthropic,
                model: "test-model".to_string(),
                usage: Usage::default(),
                stop_reason: StopReason::Stop,
                error_message: None,
                timestamp: Utc::now().timestamp_millis(),
            },
        ))];

        let html = render_html(&messages, "Thinking");
        assert!(html.contains("class=\"thinking\""));
        assert!(html.contains("Let me think about this..."));
        assert!(html.contains("Here is my answer."));
    }

    #[tokio::test]
    async fn export_to_html_creates_file() {
        let dir = std::env::temp_dir().join(format!("pi-rs-export-test-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&dir)
            .await
            .expect("create temp dir");
        let output_path = dir.join("export.html");

        let messages = vec![
            make_user_agent_message("What is Rust?"),
            make_assistant_agent_message("Rust is a systems programming language."),
        ];

        let result = export_to_html(&messages, Some(&output_path), Some("Test Export"))
            .await
            .expect("export should succeed");

        assert_eq!(result, output_path);
        assert!(output_path.exists());

        let content = tokio::fs::read_to_string(&output_path)
            .await
            .expect("read exported file");
        assert!(content.contains("<!DOCTYPE html>"));
        assert!(content.contains("Test Export"));
        assert!(content.contains("What is Rust?"));
        assert!(content.contains("Rust is a systems programming language."));

        tokio::fs::remove_dir_all(dir).await.ok();
    }

    #[tokio::test]
    async fn export_to_html_uses_default_path_when_none() {
        let messages = vec![make_user_agent_message("test")];

        let result = export_to_html(&messages, None, None)
            .await
            .expect("export with defaults");

        assert!(result.exists());
        let filename = result.file_name().unwrap().to_str().unwrap();
        assert!(filename.starts_with("session_export_"));
        assert!(filename.ends_with(".html"));

        tokio::fs::remove_file(&result).await.ok();
    }
}
