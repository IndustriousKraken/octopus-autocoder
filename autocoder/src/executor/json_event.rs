//! Parser for Claude CLI's `--output-format stream-json` event stream.
//!
//! Each line on stdout is one JSON object describing one event in the
//! agent's turn — system header, intermediate assistant text, tool calls,
//! tool results, or the closing `result` event. The parser is permissive
//! by design: unknown `type` values surface as `Unknown` so a Claude CLI
//! version that adds new event types does NOT crash the executor; only
//! truly malformed JSON returns `Err`. Neither variant aborts the
//! caller's parse loop.

use serde_json::Value;
use thiserror::Error;

/// One JSON event from Claude CLI's stream-json output.
#[derive(Debug, Clone, PartialEq)]
pub enum JsonEvent {
    /// Initial system/init metadata (model, session id, MCP server health).
    System { content: Value },
    /// An assistant turn carrying one or more content blocks. Each block
    /// is either intermediate text the model wrote before its next tool
    /// call, or a tool-use request.
    Assistant { content_blocks: Vec<AssistantBlock> },
    /// A user turn (Claude CLI's framing for tool-result deliveries — the
    /// model receives them as `user` role messages).
    User { content_blocks: Vec<UserBlock> },
    /// The terminal event for a successful turn. `final_text` is the
    /// model's concluding conversational summary; the PR-comment path
    /// reads from here.
    Result {
        stop_reason: String,
        final_text: String,
    },
    /// Forward-compat catch-all: a known JSON shape with a `type` value
    /// we don't recognize. Routed to the log as `[unknown:<type>]` so
    /// operators still see it.
    Unknown { event_type: String, raw: Value },
}

#[derive(Debug, Clone, PartialEq)]
pub enum AssistantBlock {
    Text { text: String },
    ToolUse { tool_name: String, tool_input: Value },
}

#[derive(Debug, Clone, PartialEq)]
pub enum UserBlock {
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
}

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("malformed JSON line: {underlying}")]
    MalformedJson {
        #[source]
        underlying: serde_json::Error,
    },
}

/// Parse one stream-json line into a `JsonEvent`. Returns `Err` only on
/// JSON-parse failure; unknown `type` values map to `Unknown`.
pub fn parse_event_line(line: &str) -> Result<JsonEvent, ParseError> {
    let value: Value = serde_json::from_str(line)
        .map_err(|underlying| ParseError::MalformedJson { underlying })?;
    let event_type = value
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    match event_type.as_str() {
        "system" => Ok(JsonEvent::System {
            content: value.clone(),
        }),
        "assistant" => {
            let blocks = extract_assistant_blocks(&value);
            Ok(JsonEvent::Assistant {
                content_blocks: blocks,
            })
        }
        "user" => {
            let blocks = extract_user_blocks(&value);
            Ok(JsonEvent::User {
                content_blocks: blocks,
            })
        }
        "result" => {
            // Claude CLI puts the closing text in the top-level `result`
            // field. `subtype` carries the stop_reason equivalent
            // (`success`, `error_max_turns`, etc.); some versions put the
            // reason on `stop_reason`. We accept either.
            let stop_reason = value
                .get("stop_reason")
                .and_then(|v| v.as_str())
                .or_else(|| value.get("subtype").and_then(|v| v.as_str()))
                .unwrap_or("")
                .to_string();
            let final_text = value
                .get("result")
                .and_then(|v| v.as_str())
                .or_else(|| value.get("final_text").and_then(|v| v.as_str()))
                .unwrap_or("")
                .to_string();
            Ok(JsonEvent::Result {
                stop_reason,
                final_text,
            })
        }
        other => Ok(JsonEvent::Unknown {
            event_type: other.to_string(),
            raw: value,
        }),
    }
}

fn extract_assistant_blocks(value: &Value) -> Vec<AssistantBlock> {
    let arr = value
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
        .or_else(|| value.get("content").and_then(|c| c.as_array()));
    let arr = match arr {
        Some(a) => a,
        None => return Vec::new(),
    };
    let mut out = Vec::with_capacity(arr.len());
    for block in arr {
        let kind = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match kind {
            "text" => {
                let text = block
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                out.push(AssistantBlock::Text { text });
            }
            "tool_use" => {
                let tool_name = block
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let tool_input = block.get("input").cloned().unwrap_or(Value::Null);
                out.push(AssistantBlock::ToolUse {
                    tool_name,
                    tool_input,
                });
            }
            _ => {
                // Unknown block type within an assistant message — silently
                // drop. The unknown-event-type path handles forward-compat
                // for the top-level event; per-block subtypes are rare and
                // not worth a separate variant.
            }
        }
    }
    out
}

fn extract_user_blocks(value: &Value) -> Vec<UserBlock> {
    let arr = value
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
        .or_else(|| value.get("content").and_then(|c| c.as_array()));
    let arr = match arr {
        Some(a) => a,
        None => return Vec::new(),
    };
    let mut out = Vec::with_capacity(arr.len());
    for block in arr {
        let kind = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if kind != "tool_result" {
            continue;
        }
        let tool_use_id = block
            .get("tool_use_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        // `content` may be a plain string OR a list of `{type:"text",text:"..."}` blocks.
        let content = match block.get("content") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Array(items)) => {
                let mut buf = String::new();
                for item in items {
                    if let Some(t) = item.get("text").and_then(|v| v.as_str()) {
                        if !buf.is_empty() {
                            buf.push('\n');
                        }
                        buf.push_str(t);
                    }
                }
                buf
            }
            Some(other) => other.to_string(),
            None => String::new(),
        };
        let is_error = block
            .get("is_error")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        out.push(UserBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assistant_text_block() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hello"}]}}"#;
        let ev = parse_event_line(line).unwrap();
        match ev {
            JsonEvent::Assistant { content_blocks } => {
                assert_eq!(content_blocks.len(), 1);
                assert_eq!(
                    content_blocks[0],
                    AssistantBlock::Text {
                        text: "hello".into()
                    }
                );
            }
            other => panic!("expected Assistant, got {other:?}"),
        }
    }

    #[test]
    fn assistant_tool_use_block() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"path":"src/foo.rs"}}]}}"#;
        let ev = parse_event_line(line).unwrap();
        match ev {
            JsonEvent::Assistant { content_blocks } => {
                assert_eq!(content_blocks.len(), 1);
                match &content_blocks[0] {
                    AssistantBlock::ToolUse {
                        tool_name,
                        tool_input,
                    } => {
                        assert_eq!(tool_name, "Read");
                        assert_eq!(
                            tool_input.get("path").and_then(|v| v.as_str()),
                            Some("src/foo.rs")
                        );
                    }
                    other => panic!("expected ToolUse, got {other:?}"),
                }
            }
            other => panic!("expected Assistant, got {other:?}"),
        }
    }

    #[test]
    fn user_tool_result_block() {
        let line = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"abc","content":"file contents...","is_error":false}]}}"#;
        let ev = parse_event_line(line).unwrap();
        match ev {
            JsonEvent::User { content_blocks } => {
                assert_eq!(
                    content_blocks[0],
                    UserBlock::ToolResult {
                        tool_use_id: "abc".into(),
                        content: "file contents...".into(),
                        is_error: false,
                    }
                );
            }
            other => panic!("expected User, got {other:?}"),
        }
    }

    #[test]
    fn user_tool_result_with_array_content() {
        // Claude CLI sometimes emits tool_result.content as an array of
        // text-block sub-objects; flatten them into a single string.
        let line = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"abc","content":[{"type":"text","text":"line1"},{"type":"text","text":"line2"}],"is_error":false}]}}"#;
        let ev = parse_event_line(line).unwrap();
        match ev {
            JsonEvent::User { content_blocks } => match &content_blocks[0] {
                UserBlock::ToolResult { content, .. } => {
                    assert!(content.contains("line1"));
                    assert!(content.contains("line2"));
                }
            },
            other => panic!("expected User, got {other:?}"),
        }
    }

    #[test]
    fn result_event() {
        let line = r#"{"type":"result","stop_reason":"end_turn","result":"I've fixed the bug..."}"#;
        let ev = parse_event_line(line).unwrap();
        match ev {
            JsonEvent::Result {
                stop_reason,
                final_text,
            } => {
                assert_eq!(stop_reason, "end_turn");
                assert_eq!(final_text, "I've fixed the bug...");
            }
            other => panic!("expected Result, got {other:?}"),
        }
    }

    #[test]
    fn unknown_event_type_routes_to_unknown_variant() {
        let line = r#"{"type":"future_event_kind","foo":"bar"}"#;
        let ev = parse_event_line(line).unwrap();
        match ev {
            JsonEvent::Unknown { event_type, raw } => {
                assert_eq!(event_type, "future_event_kind");
                assert_eq!(raw.get("foo").and_then(|v| v.as_str()), Some("bar"));
            }
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    #[test]
    fn malformed_json_returns_err() {
        let line = "{this is not json";
        let err = parse_event_line(line).expect_err("malformed JSON should error");
        match err {
            ParseError::MalformedJson { .. } => {}
        }
    }

    #[test]
    fn system_event_carries_raw_value() {
        let line = r#"{"type":"system","subtype":"init","session_id":"abc"}"#;
        let ev = parse_event_line(line).unwrap();
        match ev {
            JsonEvent::System { content } => {
                assert_eq!(
                    content.get("session_id").and_then(|v| v.as_str()),
                    Some("abc")
                );
            }
            other => panic!("expected System, got {other:?}"),
        }
    }

    #[test]
    fn result_event_accepts_subtype_as_stop_reason() {
        // Some Claude CLI versions emit `subtype` instead of `stop_reason`.
        let line = r#"{"type":"result","subtype":"success","result":"done"}"#;
        let ev = parse_event_line(line).unwrap();
        match ev {
            JsonEvent::Result {
                stop_reason,
                final_text,
            } => {
                assert_eq!(stop_reason, "success");
                assert_eq!(final_text, "done");
            }
            other => panic!("expected Result, got {other:?}"),
        }
    }
}
