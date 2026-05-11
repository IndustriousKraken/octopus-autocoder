//! Minimal stdio MCP server exposing one tool: `ask_user(question)`.
//!
//! Launched by `claude-cli` (or any MCP-compatible CLI agent) as a child
//! process via the workspace's `.mcp.json` configuration written by
//! `ClaudeCliExecutor` at run time. When the wrapped agent invokes
//! `ask_user`, this server writes
//! `<workspace>/openspec/changes/<change>/.askuser-pending.json` containing
//! the question, then returns a successful tool-call result so the agent
//! sees its tool succeeded. The orchestrator picks up the marker file
//! after the child process exits.
//!
//! Protocol: JSON-RPC 2.0 over stdio with newline-delimited messages.
//! Only the subset needed by Claude Code's MCP client is implemented:
//! `initialize`, `tools/list`, `tools/call`.

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

/// Env vars the orchestrator sets in the MCP server child's environment so
/// the server knows where to write the marker file.
pub const ENV_WORKSPACE: &str = "ORCH_MCP_WORKSPACE";
pub const ENV_CHANGE: &str = "ORCH_MCP_CHANGE";

/// Run the stdio MCP server until stdin closes. Returns Ok on a clean
/// shutdown (EOF on stdin) or Err on a protocol/IO failure.
pub fn run() -> Result<()> {
    let workspace = std::env::var(ENV_WORKSPACE)
        .with_context(|| format!("missing {ENV_WORKSPACE} in MCP server env"))?;
    let change = std::env::var(ENV_CHANGE)
        .with_context(|| format!("missing {ENV_CHANGE} in MCP server env"))?;
    let marker_path = PathBuf::from(&workspace)
        .join("openspec/changes")
        .join(&change)
        .join(".askuser-pending.json");

    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let stdout = std::io::stdout();
    let mut writer = stdout.lock();

    loop {
        let mut line = String::new();
        let n = reader
            .read_line(&mut line)
            .context("reading from stdin")?;
        if n == 0 {
            // EOF: peer closed stdin.
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let req: JsonRpcRequest = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(e) => {
                // Malformed JSON — emit a parse error and continue listening.
                emit_error(&mut writer, None, -32700, &format!("parse error: {e}"))?;
                continue;
            }
        };
        handle_request(&mut writer, &marker_path, req)?;
    }
    Ok(())
}

fn handle_request<W: Write>(
    writer: &mut W,
    marker_path: &std::path::Path,
    req: JsonRpcRequest,
) -> Result<()> {
    // JSON-RPC 2.0: requests with `id` expect a response; notifications
    // (no `id`) do not. We emit a response for every request that has an id.
    let id = req.id.clone();
    match req.method.as_str() {
        "initialize" => {
            let result = serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": "orchestrator-ask-user",
                    "version": env!("CARGO_PKG_VERSION"),
                }
            });
            emit_result(writer, id, result)?;
        }
        "notifications/initialized" => {
            // Notification — no response expected.
        }
        "tools/list" => {
            let result = serde_json::json!({
                "tools": [
                    {
                        "name": "ask_user",
                        "description": "Ask the human operator a question when you cannot proceed without their input. After calling this tool, stop further changes; the orchestrator will deliver the human's answer in a subsequent invocation.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "question": {
                                    "type": "string",
                                    "description": "A clear, self-contained question to ask the human."
                                }
                            },
                            "required": ["question"]
                        }
                    }
                ]
            });
            emit_result(writer, id, result)?;
        }
        "tools/call" => {
            let params = req
                .params
                .ok_or_else(|| anyhow!("tools/call missing params"))?;
            let call: ToolCallParams = serde_json::from_value(params)
                .map_err(|e| anyhow!("tools/call params decode: {e}"))?;
            if call.name != "ask_user" {
                emit_error(
                    writer,
                    id,
                    -32601,
                    &format!("unknown tool `{}`", call.name),
                )?;
                return Ok(());
            }
            let question = call
                .arguments
                .get("question")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .ok_or_else(|| anyhow!("ask_user: missing string `question` argument"))?;
            write_marker(marker_path, &question)?;
            let result = serde_json::json!({
                "content": [
                    {
                        "type": "text",
                        "text": "Your question has been delivered to the human operator. The orchestrator will resume you with their answer in a subsequent invocation. Stop further changes now."
                    }
                ],
                "isError": false
            });
            emit_result(writer, id, result)?;
        }
        other => {
            emit_error(writer, id, -32601, &format!("method not found: {other}"))?;
        }
    }
    Ok(())
}

fn write_marker(marker_path: &std::path::Path, question: &str) -> Result<()> {
    let parent = marker_path
        .parent()
        .ok_or_else(|| anyhow!("marker path has no parent: {}", marker_path.display()))?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("creating {}", parent.display()))?;
    let payload = serde_json::json!({ "question": question });
    let tmp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("creating tempfile in {}", parent.display()))?;
    serde_json::to_writer_pretty(&tmp, &payload)
        .context("serializing askuser marker")?;
    tmp.persist(marker_path)
        .map_err(|e| anyhow!("persisting marker file {}: {e}", marker_path.display()))?;
    Ok(())
}

fn emit_result<W: Write>(
    writer: &mut W,
    id: Option<serde_json::Value>,
    result: serde_json::Value,
) -> Result<()> {
    if id.is_none() {
        return Ok(()); // notification — no response
    }
    let resp = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    });
    write_message(writer, &resp)
}

fn emit_error<W: Write>(
    writer: &mut W,
    id: Option<serde_json::Value>,
    code: i64,
    message: &str,
) -> Result<()> {
    if id.is_none() {
        return Ok(());
    }
    let resp = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    });
    write_message(writer, &resp)
}

fn write_message<W: Write>(writer: &mut W, value: &serde_json::Value) -> Result<()> {
    let line = serde_json::to_string(value)?;
    writer.write_all(line.as_bytes())?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    #[allow(dead_code)]
    jsonrpc: String,
    method: String,
    #[serde(default)]
    params: Option<serde_json::Value>,
    #[serde(default)]
    id: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ToolCallParams {
    name: String,
    #[serde(default)]
    arguments: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Drive the server's `handle_request` with a sequence of synthetic
    /// JSON-RPC messages and return everything written to the response
    /// buffer. Bypasses stdin/stdout for hermetic testing.
    fn run_with(
        marker_path: &std::path::Path,
        messages: &[&str],
    ) -> Vec<serde_json::Value> {
        let mut output = Vec::<u8>::new();
        for line in messages {
            let req: JsonRpcRequest = serde_json::from_str(line).unwrap();
            handle_request(&mut output, marker_path, req).unwrap();
        }
        // Parse newline-delimited JSON responses.
        std::str::from_utf8(&output)
            .unwrap()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn initialize_returns_capabilities() {
        let dir = TempDir::new().unwrap();
        let marker = dir.path().join("openspec/changes/x/.askuser-pending.json");
        let resps = run_with(
            &marker,
            &[r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#],
        );
        assert_eq!(resps.len(), 1);
        assert_eq!(resps[0]["id"], 1);
        assert_eq!(resps[0]["result"]["serverInfo"]["name"], "orchestrator-ask-user");
        assert!(resps[0]["result"]["capabilities"]["tools"].is_object());
    }

    #[test]
    fn tools_list_returns_ask_user_tool() {
        let dir = TempDir::new().unwrap();
        let marker = dir.path().join("openspec/changes/x/.askuser-pending.json");
        let resps = run_with(
            &marker,
            &[r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#],
        );
        let tools = resps[0]["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "ask_user");
        assert!(tools[0]["inputSchema"]["properties"]["question"].is_object());
        let required: Vec<&str> = tools[0]["inputSchema"]["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(required, vec!["question"]);
    }

    #[test]
    fn tools_call_ask_user_writes_marker_file() {
        let dir = TempDir::new().unwrap();
        let marker = dir.path().join("openspec/changes/feature/.askuser-pending.json");
        let resps = run_with(
            &marker,
            &[r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"ask_user","arguments":{"question":"What should we name the project?"}}}"#],
        );
        assert_eq!(resps[0]["id"], 3);
        assert_eq!(resps[0]["result"]["isError"], false);

        assert!(marker.is_file(), "marker file must be written");
        let contents: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&marker).unwrap()).unwrap();
        assert_eq!(contents["question"], "What should we name the project?");
    }

    #[test]
    fn tools_call_unknown_tool_returns_error() {
        let dir = TempDir::new().unwrap();
        let marker = dir.path().join("x/.askuser-pending.json");
        let resps = run_with(
            &marker,
            &[r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"banana","arguments":{}}}"#],
        );
        assert_eq!(resps[0]["id"], 4);
        let err = &resps[0]["error"];
        assert_eq!(err["code"], -32601);
        assert!(err["message"].as_str().unwrap().contains("banana"));
    }

    #[test]
    fn notifications_initialized_emits_no_response() {
        let dir = TempDir::new().unwrap();
        let marker = dir.path().join("x/.askuser-pending.json");
        let resps = run_with(
            &marker,
            &[r#"{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}"#],
        );
        assert!(resps.is_empty(), "notifications must not produce responses");
    }

    #[test]
    fn unknown_method_returns_error_response() {
        let dir = TempDir::new().unwrap();
        let marker = dir.path().join("x/.askuser-pending.json");
        let resps = run_with(
            &marker,
            &[r#"{"jsonrpc":"2.0","id":5,"method":"resources/list"}"#],
        );
        assert_eq!(resps[0]["error"]["code"], -32601);
    }
}
