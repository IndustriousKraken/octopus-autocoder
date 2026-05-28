//! Minimal stdio MCP server exposing two tools (a21):
//! - `ask_user(question)` — writes a marker file the parent autocoder
//!   process picks up after the wrapped agent exits.
//! - `query_canonical_specs(query, top_k?)` — relays the request to the
//!   daemon via a Unix-domain control socket and returns ranked
//!   canonical-spec chunks for the wrapped agent's query.
//!
//! Launched by `claude-cli` (or any MCP-compatible CLI agent) as a child
//! process via the workspace's `.mcp.json` configuration written by
//! `ClaudeCliExecutor` at run time.
//!
//! Protocol: JSON-RPC 2.0 over stdio with newline-delimited messages.
//! Only the subset needed by Claude Code's MCP client is implemented:
//! `initialize`, `tools/list`, `tools/call`.

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Env vars autocoder sets in the MCP server child's environment.
pub const ENV_WORKSPACE: &str = "ORCH_MCP_WORKSPACE";
pub const ENV_CHANGE: &str = "ORCH_MCP_CHANGE";
/// Path to the daemon's control socket. Set when canonical_rag is
/// configured; absent → the `query_canonical_specs` tool returns
/// `{ hits: [], error_hint: "rag not configured for this execution" }`.
pub const ENV_CONTROL_SOCKET: &str = "ORCH_DAEMON_CONTROL_SOCKET";
/// Sanitized workspace basename routed into the control-socket request
/// so the daemon's handler can look up the right `CanonicalRagStore`.
pub const ENV_WORKSPACE_BASENAME: &str = "ORCH_MCP_WORKSPACE_BASENAME";

/// 10-second timeout for the control-socket round trip (read + write).
const CONTROL_SOCKET_TIMEOUT: Duration = Duration::from_secs(10);

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
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let req: JsonRpcRequest = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(e) => {
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
    let id = req.id.clone();
    match req.method.as_str() {
        "initialize" => {
            let result = serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": "autocoder-mcp",
                    "version": env!("AUTOCODER_VERSION"),
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
                        "description": "Ask the human operator a question when you cannot proceed without their input. After calling this tool, stop further changes; autocoder will deliver the human's answer in a subsequent invocation.",
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
                    },
                    {
                        "name": "query_canonical_specs",
                        "description": "Retrieve canonical-spec chunks for a query string via semantic similarity. Use this when you're working on a capability whose canonical contract matters. Returns ranked excerpts, not whole files; cheap to call as often as useful.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "query": {
                                    "type": "string",
                                    "description": "A search string describing what canonical-spec context you want (a requirement title, a problem you're solving, a keyword)."
                                },
                                "top_k": {
                                    "type": "integer",
                                    "description": "Optional maximum number of chunks to return. Defaults to the daemon's configured top_k (typically 10)."
                                }
                            },
                            "required": ["query"]
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
            match call.name.as_str() {
                "ask_user" => {
                    let question = call
                        .arguments
                        .get("question")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .ok_or_else(|| {
                            anyhow!("ask_user: missing string `question` argument")
                        })?;
                    write_marker(marker_path, &question)?;
                    let result = serde_json::json!({
                        "content": [
                            {
                                "type": "text",
                                "text": "Your question has been delivered to the human operator. autocoder will resume you with their answer in a subsequent invocation. Stop further changes now."
                            }
                        ],
                        "isError": false
                    });
                    emit_result(writer, id, result)?;
                }
                "query_canonical_specs" => {
                    let query_str = match call
                        .arguments
                        .get("query")
                        .and_then(|v| v.as_str())
                    {
                        Some(s) => s.to_string(),
                        None => {
                            emit_error(
                                writer,
                                id,
                                -32602,
                                "query_canonical_specs: missing string `query` argument",
                            )?;
                            return Ok(());
                        }
                    };
                    let top_k = call.arguments.get("top_k").and_then(|v| v.as_u64());
                    let payload = handle_query_canonical_specs(&query_str, top_k);
                    let result = serde_json::json!({
                        "content": [
                            {
                                "type": "text",
                                "text": serde_json::to_string(&payload)
                                    .unwrap_or_else(|_| "{}".into()),
                            }
                        ],
                        "isError": false,
                        "structuredContent": payload,
                    });
                    emit_result(writer, id, result)?;
                }
                other => {
                    emit_error(
                        writer,
                        id,
                        -32601,
                        &format!("unknown tool `{other}`"),
                    )?;
                }
            }
        }
        other => {
            emit_error(writer, id, -32601, &format!("method not found: {other}"))?;
        }
    }
    Ok(())
}

/// Build the `query_canonical_specs` tool result payload. Fail-open:
/// every error path returns `{ hits: [], error_hint: "..." }` so the
/// agent can fall back to its non-RAG behaviour gracefully.
fn handle_query_canonical_specs(
    query: &str,
    top_k: Option<u64>,
) -> serde_json::Value {
    let socket_path = match std::env::var(ENV_CONTROL_SOCKET) {
        Ok(s) => s,
        Err(_) => {
            return serde_json::json!({
                "hits": [],
                "error_hint": "rag not configured for this execution",
            });
        }
    };
    let workspace_basename = std::env::var(ENV_WORKSPACE_BASENAME).unwrap_or_default();
    let mut request = serde_json::json!({
        "action": "query_canonical_specs",
        "workspace_basename": workspace_basename,
        "query": query,
    });
    if let Some(k) = top_k {
        request["top_k"] = serde_json::json!(k);
    }
    match relay_to_control_socket(Path::new(&socket_path), &request) {
        Ok(value) => {
            // Pass through `hits` and `error_hint` from the daemon's
            // response verbatim — the daemon's fail-open contract is
            // already the right shape for the tool result.
            let hits = value
                .get("hits")
                .cloned()
                .unwrap_or_else(|| serde_json::json!([]));
            let mut out = serde_json::json!({ "hits": hits });
            if let Some(hint) = value.get("error_hint").and_then(|h| h.as_str()) {
                out["error_hint"] = serde_json::json!(hint);
            }
            out
        }
        Err(e) => serde_json::json!({
            "hits": [],
            "error_hint": format!("control socket unreachable: {e}"),
        }),
    }
}

/// Open a connection to the daemon's control socket, send `request`
/// followed by a newline, and read the single-line JSON response. Both
/// halves are bounded by `CONTROL_SOCKET_TIMEOUT`.
fn relay_to_control_socket(
    socket: &Path,
    request: &serde_json::Value,
) -> Result<serde_json::Value> {
    let stream = UnixStream::connect(socket)
        .with_context(|| format!("connecting to control socket at {}", socket.display()))?;
    stream.set_read_timeout(Some(CONTROL_SOCKET_TIMEOUT))?;
    stream.set_write_timeout(Some(CONTROL_SOCKET_TIMEOUT))?;
    let mut stream = stream;
    let raw = serde_json::to_string(request)?;
    stream.write_all(raw.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    let mut buf = String::new();
    stream.read_to_string(&mut buf)?;
    let value: serde_json::Value = serde_json::from_str(buf.trim())
        .with_context(|| format!("decoding control-socket response: {buf:?}"))?;
    Ok(value)
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
    use std::os::unix::net::UnixListener;
    use std::sync::Mutex;
    use tempfile::TempDir;

    /// Env-var mutation is global; serialize the env-var-touching tests
    /// so concurrent runs do not race.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Drive the server's `handle_request` with a sequence of synthetic
    /// JSON-RPC messages and return everything written to the response
    /// buffer.
    fn run_with(
        marker_path: &std::path::Path,
        messages: &[&str],
    ) -> Vec<serde_json::Value> {
        let mut output = Vec::<u8>::new();
        for line in messages {
            let req: JsonRpcRequest = serde_json::from_str(line).unwrap();
            handle_request(&mut output, marker_path, req).unwrap();
        }
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
        assert_eq!(resps[0]["result"]["serverInfo"]["name"], "autocoder-mcp");
        assert!(resps[0]["result"]["capabilities"]["tools"].is_object());
    }

    #[test]
    fn tools_list_advertises_both_tools() {
        let dir = TempDir::new().unwrap();
        let marker = dir.path().join("openspec/changes/x/.askuser-pending.json");
        let resps = run_with(
            &marker,
            &[r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#],
        );
        let tools = resps[0]["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"ask_user"));
        assert!(names.contains(&"query_canonical_specs"));
        let rag_tool = tools
            .iter()
            .find(|t| t["name"] == "query_canonical_specs")
            .unwrap();
        assert!(rag_tool["inputSchema"]["properties"]["query"].is_object());
        let required: Vec<&str> = rag_tool["inputSchema"]["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(required, vec!["query"]);
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

    #[test]
    fn query_canonical_specs_env_absent_returns_not_configured_hint() {
        let _g = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::remove_var(ENV_CONTROL_SOCKET);
            std::env::remove_var(ENV_WORKSPACE_BASENAME);
        }
        let dir = TempDir::new().unwrap();
        let marker = dir.path().join("x/.askuser-pending.json");
        let resps = run_with(
            &marker,
            &[r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"query_canonical_specs","arguments":{"query":"audit cadence"}}}"#],
        );
        let structured = &resps[0]["result"]["structuredContent"];
        assert!(structured["hits"].as_array().unwrap().is_empty());
        assert_eq!(
            structured["error_hint"].as_str().unwrap(),
            "rag not configured for this execution"
        );
    }

    #[test]
    fn query_canonical_specs_relays_via_socket() {
        let _g = ENV_LOCK.lock().unwrap();
        let socket_dir = TempDir::new().unwrap();
        let socket_path = socket_dir.path().join("control.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        // Spawn a thread that answers ONE request with a canned response
        // and exits.
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = String::new();
            let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
            std::io::BufRead::read_line(&mut reader, &mut buf).unwrap();
            // Echo what we got plus a fixed hits array.
            let response = serde_json::json!({
                "ok": true,
                "hits": [
                    {"capability": "audits", "requirement_title": "Audit cadence",
                     "requirement_body": "...", "scenario_titles": [], "relevance_score": 0.9}
                ],
            });
            let mut s = serde_json::to_string(&response).unwrap();
            s.push('\n');
            stream.write_all(s.as_bytes()).unwrap();
        });
        unsafe {
            std::env::set_var(ENV_CONTROL_SOCKET, socket_path.to_string_lossy().to_string());
            std::env::set_var(ENV_WORKSPACE_BASENAME, "test-ws");
        }
        let dir = TempDir::new().unwrap();
        let marker = dir.path().join("x/.askuser-pending.json");
        let resps = run_with(
            &marker,
            &[r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"query_canonical_specs","arguments":{"query":"audit cadence","top_k":3}}}"#],
        );
        handle.join().unwrap();
        let structured = &resps[0]["result"]["structuredContent"];
        let hits = structured["hits"].as_array().unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0]["capability"], "audits");
        unsafe {
            std::env::remove_var(ENV_CONTROL_SOCKET);
            std::env::remove_var(ENV_WORKSPACE_BASENAME);
        }
    }

    #[test]
    fn query_canonical_specs_socket_unreachable_returns_hint() {
        let _g = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var(ENV_CONTROL_SOCKET, "/nonexistent/control.sock");
            std::env::set_var(ENV_WORKSPACE_BASENAME, "test-ws");
        }
        let dir = TempDir::new().unwrap();
        let marker = dir.path().join("x/.askuser-pending.json");
        let resps = run_with(
            &marker,
            &[r#"{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"query_canonical_specs","arguments":{"query":"x"}}}"#],
        );
        let structured = &resps[0]["result"]["structuredContent"];
        assert!(structured["hits"].as_array().unwrap().is_empty());
        let hint = structured["error_hint"].as_str().unwrap();
        assert!(
            hint.contains("control socket unreachable"),
            "hint should name socket-unreachable; got: {hint}"
        );
        unsafe {
            std::env::remove_var(ENV_CONTROL_SOCKET);
            std::env::remove_var(ENV_WORKSPACE_BASENAME);
        }
    }
}
