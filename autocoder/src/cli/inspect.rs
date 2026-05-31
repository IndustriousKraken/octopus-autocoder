//! `autocoder inspect` — operator-friendly diagnostics that wrap the
//! daemon's existing per-change stream logs AND the control-socket
//! `query_canonical_specs` action. Three subsubcommands: `rag`, `log`,
//! `tool-usage`.
//!
//! The subcommands do not introduce new daemon state — they read existing
//! artifacts AND speak existing protocols. The CLI is a pure presentation
//! layer over data the daemon already produces.

use anyhow::{Context, Result, anyhow};
use std::io::Write;
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use super::InspectSubcommand;
use crate::paths::DaemonPaths;

/// Operator-error exit code (missing arg, unresolvable workspace,
/// unreachable socket, missing log file). Mirrors the spec.
const EXIT_OPERATOR_ERROR: i32 = 2;

/// 10-second timeout for the control-socket round trip.
const CONTROL_SOCKET_TIMEOUT_SECS: u64 = 10;

/// What the subsubcommand wants the dispatcher to do with the process.
/// Operator errors carry their own already-written-to-stderr message AND
/// hand back the canonical exit `2`. Production dispatch (`rag`, `log`,
/// `tool_usage`) translates `OperatorError` into a real `process::exit`;
/// tests inspect the enum directly.
#[derive(Debug, PartialEq, Eq)]
pub enum InspectOutcome {
    Ok,
    OperatorError,
}

impl InspectOutcome {
    fn into_result(self) -> Result<()> {
        match self {
            InspectOutcome::Ok => Ok(()),
            InspectOutcome::OperatorError => std::process::exit(EXIT_OPERATOR_ERROR),
        }
    }
}

/// Top-level dispatcher for `autocoder inspect <subsubcommand>`. Each arm
/// resolves `DaemonPaths` from the environment AND delegates to the
/// subsubcommand's executor. Operator-class errors short-circuit through
/// `std::process::exit(2)`; anyhow `Err` values propagate to `main` AND
/// surface as exit `1`.
pub async fn dispatch(command: InspectSubcommand) -> Result<()> {
    let paths = super::resolve_paths_from_env()?;
    match command {
        InspectSubcommand::Rag {
            workspace,
            query,
            top_k,
            show_bodies,
            json,
        } => {
            rag(
                &paths,
                RagArgs {
                    workspace,
                    query,
                    top_k,
                    show_bodies,
                    json,
                },
                &mut std::io::stdout().lock(),
                &mut std::io::stderr().lock(),
            )
            .await
        }
        InspectSubcommand::Log {
            workspace,
            change,
            limit,
            json,
        } => {
            log(
                &paths,
                LogArgs {
                    workspace,
                    change,
                    limit,
                    json,
                },
                &mut std::io::stdout().lock(),
                &mut std::io::stderr().lock(),
            )
            .await
        }
        InspectSubcommand::ToolUsage {
            workspace,
            change,
            json,
        } => {
            tool_usage(
                &paths,
                ToolUsageArgs {
                    workspace,
                    change,
                    json,
                },
                &mut std::io::stdout().lock(),
                &mut std::io::stderr().lock(),
            )
            .await
        }
    }
}

// =====================================================================
// Workspace resolution
// =====================================================================

/// Resolve `--workspace` to a sanitized basename:
///   - When `Some(s)` with a `:` OR `http`/`https` prefix: treated as a
///     URL AND sanitized via [`crate::workspace::sanitize_url`].
///   - When `Some(s)` otherwise: used as a basename verbatim.
///   - When `None`: enumerate the daemon's clone directory; pick the
///     only entry, OR list the available basenames AND return `Err`.
pub fn resolve_workspace_basename(
    paths: &DaemonPaths,
    arg: Option<String>,
) -> Result<String> {
    if let Some(s) = arg {
        let s = s.trim().to_string();
        if s.is_empty() {
            return Err(anyhow!(
                "--workspace: empty string is not a valid basename"
            ));
        }
        if looks_like_url(&s) {
            return Ok(crate::workspace::sanitize_url(&s));
        }
        return Ok(s);
    }
    let basenames = list_workspace_basenames(paths);
    match basenames.as_slice() {
        [only] => Ok(only.clone()),
        [] => Err(anyhow!(
            "--workspace required. No workspaces found under {}.",
            paths.workspaces_dir().display(),
        )),
        many => Err(anyhow!(
            "--workspace required. Available basenames: {}",
            many.join(", ")
        )),
    }
}

fn looks_like_url(s: &str) -> bool {
    s.contains(':') || s.starts_with("http://") || s.starts_with("https://")
}

/// Enumerate `<cache>/workspaces/<basename>/` directories. The daemon
/// derives one per configured repository, so the basenames here are the
/// same identifiers `inspect log` AND `inspect rag` accept.
fn list_workspace_basenames(paths: &DaemonPaths) -> Vec<String> {
    let dir = paths.workspaces_dir();
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return out;
    };
    for entry in rd.flatten() {
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        if !ft.is_dir() {
            continue;
        }
        if let Some(name) = entry.file_name().to_str() {
            out.push(name.to_string());
        }
    }
    out.sort();
    out
}

// =====================================================================
// `inspect rag`
// =====================================================================

pub struct RagArgs {
    pub workspace: Option<String>,
    pub query: String,
    pub top_k: Option<u32>,
    pub show_bodies: bool,
    pub json: bool,
}

pub async fn rag<O: Write, E: Write>(
    paths: &DaemonPaths,
    args: RagArgs,
    stdout: &mut O,
    stderr: &mut E,
) -> Result<()> {
    let basename = match resolve_workspace_basename(paths, args.workspace.clone()) {
        Ok(b) => b,
        Err(e) => {
            writeln!(stderr, "error: {e:#}").ok();
            return InspectOutcome::OperatorError.into_result();
        }
    };
    let socket = paths.control_socket_path();
    let outcome = rag_at(
        &socket,
        &basename,
        &args.query,
        args.top_k,
        args.show_bodies,
        args.json,
        stdout,
        stderr,
    )
    .await?;
    outcome.into_result()
}

/// Connect-and-render core, separated from path resolution so tests can
/// point it at a fake Unix socket.
#[allow(clippy::too_many_arguments)]
pub async fn rag_at<O: Write, E: Write>(
    socket: &Path,
    workspace_basename: &str,
    query: &str,
    top_k: Option<u32>,
    show_bodies: bool,
    json: bool,
    stdout: &mut O,
    stderr: &mut E,
) -> Result<InspectOutcome> {
    let stream = match UnixStream::connect(socket).await {
        Ok(s) => s,
        Err(e) => {
            writeln!(
                stderr,
                "error: control socket unreachable at {}: {e}. \
                 Is the daemon running? (systemctl status autocoder)",
                socket.display(),
            )
            .ok();
            return Ok(InspectOutcome::OperatorError);
        }
    };
    let mut request = serde_json::json!({
        "action": "query_canonical_specs",
        "workspace_basename": workspace_basename,
        "query": query,
    });
    if let Some(k) = top_k {
        request["top_k"] = serde_json::json!(k);
    }
    let raw_response = round_trip(stream, &request).await?;
    if json {
        writeln!(stdout, "{}", raw_response.trim_end()).ok();
        return Ok(InspectOutcome::Ok);
    }
    let value: serde_json::Value = serde_json::from_str(raw_response.trim())
        .with_context(|| {
            format!("decoding control-socket response: {raw_response:?}")
        })?;
    render_rag(stdout, &value, query, workspace_basename, top_k, show_bodies)?;
    Ok(InspectOutcome::Ok)
}

async fn round_trip(
    stream: UnixStream,
    request: &serde_json::Value,
) -> Result<String> {
    let (read_half, mut write_half) = stream.into_split();
    let raw = serde_json::to_string(request)?;
    let timeout = Duration::from_secs(CONTROL_SOCKET_TIMEOUT_SECS);
    tokio::time::timeout(timeout, async {
        write_half.write_all(raw.as_bytes()).await?;
        write_half.write_all(b"\n").await?;
        write_half.flush().await?;
        Ok::<_, anyhow::Error>(())
    })
    .await
    .map_err(|_| anyhow!("control socket write timed out"))??;
    let mut reader = BufReader::new(read_half);
    let mut buf = String::new();
    tokio::time::timeout(timeout, reader.read_to_string(&mut buf))
        .await
        .map_err(|_| anyhow!("control socket read timed out"))??;
    Ok(buf)
}

/// Render the rag response as the canonical aligned table. Sized for a
/// reasonable 100-column terminal: SCORE (5) CAPABILITY (24)
/// REQUIREMENT (50) BYTES (8).
fn render_rag<O: Write>(
    stdout: &mut O,
    response: &serde_json::Value,
    query: &str,
    workspace: &str,
    top_k: Option<u32>,
    show_bodies: bool,
) -> Result<()> {
    let hits = response
        .get("hits")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut hits: Vec<&serde_json::Value> = hits.iter().collect();
    hits.sort_by(|a, b| {
        let sa = hit_score(a);
        let sb = hit_score(b);
        sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
    });
    writeln!(stdout, "Query: {query}").ok();
    writeln!(stdout, "Workspace: {workspace}").ok();
    if let Some(k) = top_k {
        writeln!(stdout, "top_k: {k}").ok();
    } else {
        writeln!(stdout, "top_k: (daemon default)").ok();
    }
    writeln!(stdout).ok();
    if let Some(hint) = response.get("error_hint").and_then(|v| v.as_str()) {
        writeln!(stdout, "  hint: {hint}").ok();
    }
    writeln!(
        stdout,
        "  {:<5}  {:<24}  {:<50}  {:>8}",
        "SCORE", "CAPABILITY", "REQUIREMENT", "BYTES"
    )
    .ok();
    let mut total_bytes: usize = 0;
    for hit in &hits {
        let score = hit_score(hit);
        let capability = hit
            .get("capability")
            .and_then(|v| v.as_str())
            .unwrap_or("(unknown)");
        let requirement = hit
            .get("requirement_title")
            .and_then(|v| v.as_str())
            .unwrap_or("(unknown)");
        let bytes = hit
            .get("requirement_body")
            .and_then(|v| v.as_str())
            .map(|s| s.len())
            .unwrap_or(0);
        total_bytes += bytes;
        writeln!(
            stdout,
            "  {:<5.3}  {:<24}  {:<50}  {:>8}",
            score,
            truncate_for_column(capability, 24),
            truncate_for_column(requirement, 50),
            bytes
        )
        .ok();
    }
    writeln!(stdout).ok();
    let total_bytes_kb = (total_bytes as f64) / 1024.0;
    writeln!(
        stdout,
        "Total response size: {:.1} KB ({} hits)",
        total_bytes_kb,
        hits.len()
    )
    .ok();
    if show_bodies {
        for hit in &hits {
            let capability = hit
                .get("capability")
                .and_then(|v| v.as_str())
                .unwrap_or("(unknown)");
            let requirement = hit
                .get("requirement_title")
                .and_then(|v| v.as_str())
                .unwrap_or("(unknown)");
            let body = hit
                .get("requirement_body")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            writeln!(stdout).ok();
            writeln!(stdout, "## {capability}/{requirement}").ok();
            let snippet: String = body.chars().take(500).collect();
            writeln!(stdout, "{snippet}").ok();
        }
    }
    Ok(())
}

fn hit_score(hit: &serde_json::Value) -> f64 {
    hit.get("relevance_score")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0)
}

fn truncate_for_column(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        return s.to_string();
    }
    if width <= 1 {
        return "…".to_string();
    }
    let mut out: String = s.chars().take(width - 1).collect();
    out.push('…');
    out
}

// =====================================================================
// `inspect log`
// =====================================================================

pub struct LogArgs {
    pub workspace: Option<String>,
    pub change: String,
    pub limit: Option<u32>,
    pub json: bool,
}

pub async fn log<O: Write, E: Write>(
    paths: &DaemonPaths,
    args: LogArgs,
    stdout: &mut O,
    stderr: &mut E,
) -> Result<()> {
    log_inner(paths, args, stdout, stderr).await?.into_result()
}

async fn log_inner<O: Write, E: Write>(
    paths: &DaemonPaths,
    args: LogArgs,
    stdout: &mut O,
    stderr: &mut E,
) -> Result<InspectOutcome> {
    let basename = match resolve_workspace_basename(paths, args.workspace.clone()) {
        Ok(b) => b,
        Err(e) => {
            writeln!(stderr, "error: {e:#}").ok();
            return Ok(InspectOutcome::OperatorError);
        }
    };
    let runs_dir = paths.run_logs_dir(&basename);
    let stream_log_path = runs_dir.join(format!("{}.stream.log", args.change));
    let summary_log_path = runs_dir.join(format!("{}.log", args.change));
    if !stream_log_path.is_file() {
        let available = list_available_changes(&runs_dir);
        let list = if available.is_empty() {
            "(none)".to_string()
        } else {
            available.join(", ")
        };
        writeln!(
            stderr,
            "error: no stream log at {}. Available changes in this workspace: {}",
            stream_log_path.display(),
            list
        )
        .ok();
        return Ok(InspectOutcome::OperatorError);
    }
    let raw = std::fs::read_to_string(&stream_log_path)
        .with_context(|| format!("reading stream log {}", stream_log_path.display()))?;
    let events = parse_stream_log(&raw);
    if args.json {
        let arr = serde_json::to_string_pretty(&events)?;
        writeln!(stdout, "{arr}").ok();
        return Ok(InspectOutcome::Ok);
    }
    let limit = match args.limit {
        Some(0) => None,
        Some(n) => Some(n as usize),
        None => Some(30),
    };
    render_log(
        stdout,
        &args.change,
        &basename,
        &summary_log_path,
        &stream_log_path,
        &events,
        limit,
    );
    Ok(InspectOutcome::Ok)
}

fn list_available_changes(runs_dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(runs_dir) else {
        return out;
    };
    for entry in rd.flatten() {
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        if let Some(stem) = name_str.strip_suffix(".stream.log") {
            out.push(stem.to_string());
        }
    }
    out.sort();
    out
}

/// A parsed stream-log event. Only the kinds the renderer cares about
/// are typed explicitly; everything else lands as `Other`. Timestamps
/// are not present in the existing on-disk format (the executor's
/// `event_log` writer does not embed them), so the field is preserved
/// for forward-compatibility but is `None` against current logs.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StreamEvent {
    ToolUse {
        timestamp: Option<String>,
        name: String,
        input_summary: String,
    },
    ToolResult {
        timestamp: Option<String>,
        summary: String,
    },
    Assistant {
        timestamp: Option<String>,
        text: String,
    },
    Other {
        timestamp: Option<String>,
        prefix: String,
        body: String,
    },
}

impl StreamEvent {
    fn timestamp(&self) -> Option<&str> {
        match self {
            StreamEvent::ToolUse { timestamp, .. }
            | StreamEvent::ToolResult { timestamp, .. }
            | StreamEvent::Assistant { timestamp, .. }
            | StreamEvent::Other { timestamp, .. } => timestamp.as_deref(),
        }
    }
}

/// Parse a stream-log into a sequence of events. The on-disk format is
/// one event per line of the form `[<kind>] <body>`. The optional
/// `[HH:MM:SS.mmm]` timestamp prefix that the proposal anticipated is
/// recognised here but is absent from current logs.
pub fn parse_stream_log(raw: &str) -> Vec<StreamEvent> {
    let mut out = Vec::new();
    for line in raw.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let (timestamp, remainder) = split_timestamp(line);
        let Some((prefix, body)) = split_event_prefix(remainder) else {
            out.push(StreamEvent::Other {
                timestamp: timestamp.map(str::to_string),
                prefix: "raw".to_string(),
                body: remainder.to_string(),
            });
            continue;
        };
        let event = match prefix {
            "tool_use" => {
                let (name, summary) = split_tool_use_body(body);
                StreamEvent::ToolUse {
                    timestamp: timestamp.map(str::to_string),
                    name,
                    input_summary: summary,
                }
            }
            "tool_result" => StreamEvent::ToolResult {
                timestamp: timestamp.map(str::to_string),
                summary: body.to_string(),
            },
            "assistant" => StreamEvent::Assistant {
                timestamp: timestamp.map(str::to_string),
                text: body.to_string(),
            },
            other => StreamEvent::Other {
                timestamp: timestamp.map(str::to_string),
                prefix: other.to_string(),
                body: body.to_string(),
            },
        };
        out.push(event);
    }
    out
}

/// Split a leading `[HH:MM:SS.mmm]` (or any non-event bracket) timestamp
/// off the line. Returns `(Some(timestamp), rest)` only when the first
/// bracket clearly is NOT an event-kind tag. The existing executor
/// format has no timestamp; this is forward-compatible.
fn split_timestamp(line: &str) -> (Option<&str>, &str) {
    let line = line.trim_end();
    let Some(rest_after_lbrack) = line.strip_prefix('[') else {
        return (None, line);
    };
    let Some(rbrack_idx) = rest_after_lbrack.find(']') else {
        return (None, line);
    };
    let inside = &rest_after_lbrack[..rbrack_idx];
    let after = rest_after_lbrack[rbrack_idx + 1..].trim_start();
    if is_event_prefix(inside) {
        return (None, line);
    }
    if !looks_like_timestamp(inside) {
        return (None, line);
    }
    (Some(inside), after)
}

fn is_event_prefix(s: &str) -> bool {
    matches!(s, "tool_use" | "tool_result" | "assistant" | "raw")
        || s.starts_with("unknown:")
}

fn looks_like_timestamp(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_digit() || c == ':' || c == '.' || c == '-' || c == 'T')
}

fn split_event_prefix(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim_start();
    let after_lbrack = trimmed.strip_prefix('[')?;
    let rbrack = after_lbrack.find(']')?;
    let prefix = &after_lbrack[..rbrack];
    let body = after_lbrack[rbrack + 1..].trim_start();
    if !is_event_prefix(prefix) {
        return None;
    }
    Some((prefix, body))
}

/// Split a `[tool_use]` body into `(name, input_summary)`. The body
/// shape is `<tool-name>[ <input-summary>]` per the executor's
/// `format_tool_input_summary` helper.
fn split_tool_use_body(body: &str) -> (String, String) {
    let body = body.trim_start();
    match body.find(' ') {
        Some(i) => (body[..i].to_string(), body[i + 1..].trim_start().to_string()),
        None => (body.to_string(), String::new()),
    }
}

/// Extract the bare tool name from a possibly MCP-qualified name. The
/// daemon writes the qualified `mcp__<server>__<tool>` form into the
/// stream; operators care about the bare `<tool>` part for grouping.
pub fn bare_tool_name(qualified: &str) -> &str {
    qualified
        .rsplit_once("__")
        .map(|(_, t)| t)
        .unwrap_or(qualified)
}

/// Pair `[tool_use]` events with the next `[tool_result]` event in
/// source order. Stream logs don't carry a `tool_use_id`, so positional
/// pairing is the available method.
pub fn pair_tool_calls(events: &[StreamEvent]) -> Vec<ToolCallPair<'_>> {
    let mut out = Vec::new();
    let mut pending: Option<usize> = None;
    for (i, ev) in events.iter().enumerate() {
        match ev {
            StreamEvent::ToolUse { .. } => {
                if let Some(prev) = pending.take() {
                    out.push(ToolCallPair {
                        tool_use: &events[prev],
                        tool_result: None,
                    });
                }
                pending = Some(i);
            }
            StreamEvent::ToolResult { .. } => {
                if let Some(prev) = pending.take() {
                    out.push(ToolCallPair {
                        tool_use: &events[prev],
                        tool_result: Some(ev),
                    });
                } else {
                    out.push(ToolCallPair {
                        tool_use: ev,
                        tool_result: None,
                    });
                }
            }
            _ => {}
        }
    }
    if let Some(prev) = pending {
        out.push(ToolCallPair {
            tool_use: &events[prev],
            tool_result: None,
        });
    }
    out
}

#[derive(Debug)]
pub struct ToolCallPair<'a> {
    pub tool_use: &'a StreamEvent,
    pub tool_result: Option<&'a StreamEvent>,
}

fn render_log<O: Write>(
    stdout: &mut O,
    change: &str,
    workspace: &str,
    summary_log_path: &Path,
    stream_log_path: &Path,
    events: &[StreamEvent],
    limit: Option<usize>,
) {
    writeln!(stdout, "== run log: {change} ==").ok();
    writeln!(stdout, "workspace: {workspace}").ok();
    writeln!(stdout, "summary log: {}", summary_log_path.display()).ok();
    writeln!(stdout, "stream log:  {}", stream_log_path.display()).ok();
    writeln!(stdout).ok();

    let pairs = pair_tool_calls(events);
    let total = pairs.len();
    let to_render = match limit {
        Some(n) => n.min(total),
        None => total,
    };
    for pair in pairs.iter().take(to_render) {
        render_pair(stdout, pair);
        writeln!(stdout).ok();
    }
    if let Some(n) = limit
        && total > n
    {
        writeln!(
            stdout,
            "... (truncated; {total} tool calls total. Pass --limit 0 for full output.)",
        )
        .ok();
        writeln!(stdout).ok();
    }
    writeln!(stdout, "=== FINAL ANSWER ===").ok();
    match crate::executor::event_log::read_final_answer(summary_log_path) {
        Some(text) => {
            writeln!(stdout, "{text}").ok();
        }
        None => {
            writeln!(stdout, "(none recorded)").ok();
        }
    }
}

fn render_pair<O: Write>(stdout: &mut O, pair: &ToolCallPair<'_>) {
    if let StreamEvent::ToolUse {
        timestamp,
        name,
        input_summary,
    } = pair.tool_use
    {
        let ts = timestamp.as_deref().unwrap_or("--:--:--");
        let bare = bare_tool_name(name);
        let summary = format_tool_use_summary(bare, input_summary);
        writeln!(stdout, "[{ts}]  tool_use   {bare:<24}  {summary}").ok();
    }
    if let Some(StreamEvent::ToolResult { timestamp, summary }) = pair.tool_result {
        let ts = timestamp.as_deref().unwrap_or("--:--:--");
        let augmented = augment_tool_result_summary(pair.tool_use, summary);
        writeln!(stdout, "[{ts}]  tool_result                            {augmented}").ok();
    }
}

/// Format the per-tool input summary that gets rendered after the tool
/// name. For `query_canonical_specs` specifically: parse the JSON input
/// AND surface query+top_k.
fn format_tool_use_summary(bare_name: &str, raw: &str) -> String {
    if bare_name == "query_canonical_specs"
        && let Some((q, k)) = parse_query_canonical_specs_input(raw)
    {
        return match k {
            Some(top_k) => format!("query={q:?}  top_k={top_k}"),
            None => format!("query={q:?}"),
        };
    }
    raw.to_string()
}

fn parse_query_canonical_specs_input(raw: &str) -> Option<(String, Option<u64>)> {
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    let q = value.get("query").and_then(|v| v.as_str())?;
    let top_k = value.get("top_k").and_then(|v| v.as_u64());
    Some((q.to_string(), top_k))
}

/// For `query_canonical_specs` tool_result lines, parse the JSON content
/// (when present) AND append hit count + top score. The on-disk format
/// today is `(N bytes returned)` which carries no structured data; the
/// helper handles the structured form for forward compatibility AND for
/// downstream tooling that captures the raw JSON.
fn augment_tool_result_summary(tool_use: &StreamEvent, summary: &str) -> String {
    let StreamEvent::ToolUse { name, .. } = tool_use else {
        return summary.to_string();
    };
    if bare_tool_name(name) != "query_canonical_specs" {
        return summary.to_string();
    }
    if let Some((hits, top_score)) = parse_query_result_summary(summary) {
        return match top_score {
            Some(score) => format!(
                "{summary}; {hits} hit{plural}, top score {score:.3}",
                plural = if hits == 1 { "" } else { "s" },
            ),
            None => format!(
                "{summary}; {hits} hit{plural}",
                plural = if hits == 1 { "" } else { "s" },
            ),
        };
    }
    summary.to_string()
}

fn parse_query_result_summary(summary: &str) -> Option<(usize, Option<f64>)> {
    let value: serde_json::Value = serde_json::from_str(summary.trim()).ok()?;
    let hits = value.get("hits").and_then(|v| v.as_array())?;
    let top_score = hits
        .iter()
        .filter_map(|h| h.get("relevance_score").and_then(|v| v.as_f64()))
        .fold(None, |acc: Option<f64>, v| match acc {
            None => Some(v),
            Some(prev) => Some(prev.max(v)),
        });
    Some((hits.len(), top_score))
}

// =====================================================================
// `inspect tool-usage`
// =====================================================================

pub struct ToolUsageArgs {
    pub workspace: Option<String>,
    pub change: String,
    pub json: bool,
}

#[derive(Debug, Default, serde::Serialize)]
pub struct ToolUsageStats {
    pub duration: DurationStats,
    pub tool_counts: std::collections::BTreeMap<String, usize>,
    pub query_canonical_specs: Option<RagDetailStats>,
}

#[derive(Debug, Default, serde::Serialize)]
pub struct DurationStats {
    pub first_timestamp: Option<String>,
    pub last_timestamp: Option<String>,
}

#[derive(Debug, Default, serde::Serialize)]
pub struct RagDetailStats {
    pub calls: usize,
    pub total_bytes_returned: usize,
    pub total_hits: usize,
    pub high_score_hits: usize,
    pub medium_score_hits: usize,
    pub low_score_hits: usize,
    pub avg_hits_per_call: f64,
}

pub async fn tool_usage<O: Write, E: Write>(
    paths: &DaemonPaths,
    args: ToolUsageArgs,
    stdout: &mut O,
    stderr: &mut E,
) -> Result<()> {
    tool_usage_inner(paths, args, stdout, stderr)
        .await?
        .into_result()
}

async fn tool_usage_inner<O: Write, E: Write>(
    paths: &DaemonPaths,
    args: ToolUsageArgs,
    stdout: &mut O,
    stderr: &mut E,
) -> Result<InspectOutcome> {
    let basename = match resolve_workspace_basename(paths, args.workspace.clone()) {
        Ok(b) => b,
        Err(e) => {
            writeln!(stderr, "error: {e:#}").ok();
            return Ok(InspectOutcome::OperatorError);
        }
    };
    let runs_dir = paths.run_logs_dir(&basename);
    let stream_log_path = runs_dir.join(format!("{}.stream.log", args.change));
    if !stream_log_path.is_file() {
        let available = list_available_changes(&runs_dir);
        let list = if available.is_empty() {
            "(none)".to_string()
        } else {
            available.join(", ")
        };
        writeln!(
            stderr,
            "error: no stream log at {}. Available changes in this workspace: {}",
            stream_log_path.display(),
            list,
        )
        .ok();
        return Ok(InspectOutcome::OperatorError);
    }
    let raw = std::fs::read_to_string(&stream_log_path)
        .with_context(|| format!("reading stream log {}", stream_log_path.display()))?;
    let events = parse_stream_log(&raw);
    let stats = aggregate_stats(&events);
    if args.json {
        writeln!(stdout, "{}", serde_json::to_string_pretty(&stats)?).ok();
        return Ok(InspectOutcome::Ok);
    }
    render_tool_usage(stdout, &args.change, &stats);
    Ok(InspectOutcome::Ok)
}

pub fn aggregate_stats(events: &[StreamEvent]) -> ToolUsageStats {
    let mut stats = ToolUsageStats::default();

    let mut first_ts: Option<String> = None;
    let mut last_ts: Option<String> = None;
    for ev in events {
        if let Some(ts) = ev.timestamp() {
            if first_ts.is_none() {
                first_ts = Some(ts.to_string());
            }
            last_ts = Some(ts.to_string());
        }
    }
    stats.duration = DurationStats {
        first_timestamp: first_ts,
        last_timestamp: last_ts,
    };

    for ev in events {
        if let StreamEvent::ToolUse { name, .. } = ev {
            let bare = bare_tool_name(name).to_string();
            *stats.tool_counts.entry(bare).or_insert(0) += 1;
        }
    }

    let pairs = pair_tool_calls(events);
    let mut detail = RagDetailStats::default();
    let mut had_any = false;
    for pair in &pairs {
        let StreamEvent::ToolUse { name, .. } = pair.tool_use else {
            continue;
        };
        if bare_tool_name(name) != "query_canonical_specs" {
            continue;
        }
        had_any = true;
        detail.calls += 1;
        let Some(StreamEvent::ToolResult { summary, .. }) = pair.tool_result else {
            continue;
        };
        if let Some((bytes, hits, scores)) = parse_rag_result(summary) {
            detail.total_bytes_returned += bytes;
            detail.total_hits += hits;
            for s in scores {
                if s >= 0.7 {
                    detail.high_score_hits += 1;
                } else if s >= 0.5 {
                    detail.medium_score_hits += 1;
                } else {
                    detail.low_score_hits += 1;
                }
            }
        }
    }
    if detail.calls > 0 {
        detail.avg_hits_per_call =
            (detail.total_hits as f64) / (detail.calls as f64);
    }
    if had_any {
        stats.query_canonical_specs = Some(detail);
    }
    stats
}

/// Parse the tool_result summary for a `query_canonical_specs` call.
/// Returns `(bytes_returned, hit_count, scores)`. Handles both forms:
///   - Structured JSON: `{"hits":[{"relevance_score":0.9}, ...]}`
///   - Bytes-only summary: `(N bytes returned)` (current daemon format)
///     yields `(N, 0, [])` so caller can still account for bytes.
fn parse_rag_result(summary: &str) -> Option<(usize, usize, Vec<f64>)> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(summary.trim()) {
        let hits = value
            .get("hits")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let bytes = summary.len();
        let scores: Vec<f64> = hits
            .iter()
            .filter_map(|h| h.get("relevance_score").and_then(|v| v.as_f64()))
            .collect();
        return Some((bytes, hits.len(), scores));
    }
    if let Some(bytes) = parse_bytes_returned(summary) {
        return Some((bytes, 0, Vec::new()));
    }
    None
}

/// Parse a `(N bytes returned)` summary line. Returns the byte count.
fn parse_bytes_returned(summary: &str) -> Option<usize> {
    let s = summary.trim();
    let s = s.strip_prefix('(')?.strip_suffix(')')?;
    let s = s.strip_suffix(" bytes returned")?;
    s.parse::<usize>().ok()
}

fn render_tool_usage<O: Write>(
    stdout: &mut O,
    change: &str,
    stats: &ToolUsageStats,
) {
    writeln!(stdout, "== tool-usage summary: {change} ==").ok();
    let duration_line = match (
        stats.duration.first_timestamp.as_deref(),
        stats.duration.last_timestamp.as_deref(),
    ) {
        (Some(start), Some(end)) => format!("duration: (start {start}, end {end})"),
        _ => "duration: (no timestamps in stream log)".to_string(),
    };
    writeln!(stdout, "{duration_line}").ok();
    writeln!(stdout).ok();
    writeln!(stdout, "tool calls:").ok();
    if stats.tool_counts.is_empty() {
        writeln!(stdout, "  (none)").ok();
    } else {
        let max_width = stats
            .tool_counts
            .keys()
            .map(|k| k.len())
            .max()
            .unwrap_or(0);
        let mut by_count: Vec<(&String, &usize)> = stats.tool_counts.iter().collect();
        by_count.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
        for (name, count) in by_count {
            writeln!(stdout, "  {name:<width$}  {count:>5}", width = max_width).ok();
        }
    }
    if let Some(detail) = stats.query_canonical_specs.as_ref() {
        writeln!(stdout).ok();
        writeln!(stdout, "query_canonical_specs detail:").ok();
        writeln!(stdout, "  {} calls", detail.calls).ok();
        let kb = (detail.total_bytes_returned as f64) / 1024.0;
        writeln!(stdout, "  total bytes returned: {kb:.1} KB").ok();
        writeln!(stdout, "  total prompt-context loaded: {kb:.1} KB (no client-side caching)").ok();
        writeln!(stdout, "  score distribution:").ok();
        writeln!(
            stdout,
            "    high (>=0.7):   {} hits across {} calls",
            detail.high_score_hits, detail.calls
        )
        .ok();
        writeln!(
            stdout,
            "    medium (.5-.7): {} hits across {} calls",
            detail.medium_score_hits, detail.calls
        )
        .ok();
        writeln!(
            stdout,
            "    low (<.5):      {} hits across {} calls",
            detail.low_score_hits, detail.calls
        )
        .ok();
        writeln!(
            stdout,
            "  avg hits/call: {:.1}",
            detail.avg_hits_per_call
        )
        .ok();
    }
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener as StdUnixListener;
    use std::path::PathBuf;

    fn paths_with_workspaces(root: &Path, basenames: &[&str]) -> DaemonPaths {
        let paths = DaemonPaths::under_root(root);
        let ws_dir = paths.workspaces_dir();
        std::fs::create_dir_all(&ws_dir).unwrap();
        for b in basenames {
            std::fs::create_dir_all(ws_dir.join(b)).unwrap();
        }
        paths
    }

    // ---- workspace resolution helpers ----

    #[test]
    fn resolve_workspace_basename_url_arg_sanitizes() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = paths_with_workspaces(tmp.path(), &[]);
        let got = resolve_workspace_basename(
            &paths,
            Some("git@github.com:foo/bar.git".to_string()),
        )
        .unwrap();
        assert_eq!(got, "github_com_foo_bar");
    }

    #[test]
    fn resolve_workspace_basename_https_url_arg_sanitizes() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = paths_with_workspaces(tmp.path(), &[]);
        let got = resolve_workspace_basename(
            &paths,
            Some("https://github.com/foo/bar.git".to_string()),
        )
        .unwrap();
        assert_eq!(got, "github_com_foo_bar");
    }

    #[test]
    fn resolve_workspace_basename_plain_basename_passthrough() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = paths_with_workspaces(tmp.path(), &[]);
        let got = resolve_workspace_basename(
            &paths,
            Some("github_com_owner_repo".to_string()),
        )
        .unwrap();
        assert_eq!(got, "github_com_owner_repo");
    }

    #[test]
    fn resolve_workspace_basename_omit_with_single_workspace_picks_it() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = paths_with_workspaces(tmp.path(), &["github_com_only_one"]);
        let got = resolve_workspace_basename(&paths, None).unwrap();
        assert_eq!(got, "github_com_only_one");
    }

    #[test]
    fn resolve_workspace_basename_omit_with_multi_lists_them() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = paths_with_workspaces(
            tmp.path(),
            &["github_com_a_b", "github_com_c_d", "gitlab_com_e_f"],
        );
        let err = resolve_workspace_basename(&paths, None).expect_err("must err");
        let msg = format!("{err:#}");
        assert!(msg.contains("github_com_a_b"), "{msg}");
        assert!(msg.contains("github_com_c_d"), "{msg}");
        assert!(msg.contains("gitlab_com_e_f"), "{msg}");
        assert!(msg.contains("Available basenames"), "{msg}");
    }

    #[test]
    fn resolve_workspace_basename_omit_with_none_errors() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = paths_with_workspaces(tmp.path(), &[]);
        let err = resolve_workspace_basename(&paths, None).expect_err("must err");
        let msg = format!("{err:#}");
        assert!(msg.contains("No workspaces"), "{msg}");
    }

    // ---- rag rendering ----

    #[test]
    fn render_rag_table_includes_columns_and_summary() {
        let response = serde_json::json!({
            "ok": true,
            "hits": [
                {
                    "capability": "executor",
                    "requirement_title": "Outcome tools take precedence",
                    "requirement_body": "x".repeat(4123),
                    "scenario_titles": [],
                    "relevance_score": 0.847_f32,
                },
                {
                    "capability": "orchestrator-cli",
                    "requirement_title": "Control socket exposes record_outcome",
                    "requirement_body": "y".repeat(2944),
                    "scenario_titles": [],
                    "relevance_score": 0.681_f32,
                }
            ]
        });
        let mut out = Vec::new();
        render_rag(
            &mut out,
            &response,
            "outcome sentinel parsing",
            "github_com_foo_bar",
            Some(5),
            false,
        )
        .unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("Query: outcome sentinel parsing"));
        assert!(s.contains("Workspace: github_com_foo_bar"));
        assert!(s.contains("top_k: 5"));
        assert!(s.contains("SCORE"));
        assert!(s.contains("CAPABILITY"));
        assert!(s.contains("REQUIREMENT"));
        assert!(s.contains("BYTES"));
        assert!(s.contains("0.847"));
        assert!(s.contains("executor"));
        assert!(s.contains("Outcome tools"));
        assert!(s.contains("Total response size:"));
        assert!(s.contains("(2 hits)"));
    }

    #[test]
    fn render_rag_show_bodies_appends_sections() {
        let response = serde_json::json!({
            "ok": true,
            "hits": [{
                "capability": "executor",
                "requirement_title": "Sentinel parsing",
                "requirement_body": "BODY-TEXT-SHOULD-APPEAR",
                "scenario_titles": [],
                "relevance_score": 0.9_f32,
            }],
        });
        let mut out = Vec::new();
        render_rag(&mut out, &response, "q", "ws", Some(1), true).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("## executor/Sentinel parsing"), "{s}");
        assert!(s.contains("BODY-TEXT-SHOULD-APPEAR"), "{s}");
    }

    #[test]
    fn render_rag_sorts_hits_descending() {
        let response = serde_json::json!({
            "ok": true,
            "hits": [
                {"capability":"low","requirement_title":"L","requirement_body":"","scenario_titles":[],"relevance_score":0.1_f32},
                {"capability":"high","requirement_title":"H","requirement_body":"","scenario_titles":[],"relevance_score":0.9_f32},
                {"capability":"mid","requirement_title":"M","requirement_body":"","scenario_titles":[],"relevance_score":0.5_f32},
            ]
        });
        let mut out = Vec::new();
        render_rag(&mut out, &response, "q", "ws", None, false).unwrap();
        let s = String::from_utf8(out).unwrap();
        let pos_h = s.find("0.900").unwrap();
        let pos_m = s.find("0.500").unwrap();
        let pos_l = s.find("0.100").unwrap();
        assert!(pos_h < pos_m && pos_m < pos_l, "rendered: {s}");
    }

    // ---- rag socket round-trip (mocked) ----

    fn spawn_mock_socket(
        socket: PathBuf,
        response: String,
    ) -> std::thread::JoinHandle<String> {
        let listener = StdUnixListener::bind(&socket).unwrap();
        std::thread::spawn(move || {
            use std::io::{BufRead, BufReader, Write};
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = String::new();
            {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                reader.read_line(&mut buf).unwrap();
            }
            let mut body = response.into_bytes();
            if !body.ends_with(b"\n") {
                body.push(b'\n');
            }
            stream.write_all(&body).unwrap();
            stream.shutdown(std::net::Shutdown::Write).ok();
            buf
        })
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rag_at_renders_table_against_mock_socket() {
        let dir = tempfile::TempDir::new().unwrap();
        let socket = dir.path().join("control.sock");
        let response = serde_json::json!({
            "ok": true,
            "hits": [{
                "capability": "audits",
                "requirement_title": "Audit cadence",
                "requirement_body": "...".to_string(),
                "scenario_titles": [],
                "relevance_score": 0.91_f32,
            }],
        });
        let handle = spawn_mock_socket(socket.clone(), response.to_string());
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let outcome = rag_at(
            &socket,
            "github_com_foo_bar",
            "audit cadence",
            Some(3),
            false,
            false,
            &mut stdout,
            &mut stderr,
        )
        .await
        .unwrap();
        assert_eq!(outcome, InspectOutcome::Ok);
        let req = handle.join().unwrap();
        let req_json: serde_json::Value = serde_json::from_str(req.trim()).unwrap();
        assert_eq!(req_json["action"], "query_canonical_specs");
        assert_eq!(req_json["workspace_basename"], "github_com_foo_bar");
        assert_eq!(req_json["query"], "audit cadence");
        assert_eq!(req_json["top_k"], 3);
        let s = String::from_utf8(stdout).unwrap();
        assert!(s.contains("audits"), "{s}");
        assert!(s.contains("Audit cadence"), "{s}");
        assert!(s.contains("0.910"), "{s}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rag_at_json_mode_emits_raw_response() {
        let dir = tempfile::TempDir::new().unwrap();
        let socket = dir.path().join("control.sock");
        let response = serde_json::json!({
            "ok": true,
            "hits": [{
                "capability": "audits",
                "requirement_title": "Audit cadence",
                "requirement_body": "...",
                "scenario_titles": [],
                "relevance_score": 0.91_f32,
            }],
        });
        let raw = response.to_string();
        let handle = spawn_mock_socket(socket.clone(), raw.clone());
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let outcome = rag_at(
            &socket,
            "github_com_foo_bar",
            "audit cadence",
            Some(3),
            false,
            true,
            &mut stdout,
            &mut stderr,
        )
        .await
        .unwrap();
        assert_eq!(outcome, InspectOutcome::Ok);
        handle.join().unwrap();
        let s = String::from_utf8(stdout).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(s.trim()).unwrap();
        assert_eq!(parsed["hits"][0]["capability"], "audits");
        // Plain text mode would have emitted SCORE/Workspace; verify absence.
        assert!(!s.contains("SCORE"), "json mode must not render the table: {s}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rag_at_unreachable_socket_yields_operator_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let socket = dir.path().join("nope.sock"); // never bound
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let outcome = rag_at(
            &socket,
            "github_com_foo_bar",
            "q",
            Some(1),
            false,
            false,
            &mut stdout,
            &mut stderr,
        )
        .await
        .unwrap();
        assert_eq!(outcome, InspectOutcome::OperatorError);
        let err_text = String::from_utf8(stderr).unwrap();
        assert!(
            err_text.contains("control socket unreachable"),
            "{err_text}"
        );
        assert!(
            err_text.contains(socket.to_string_lossy().as_ref()),
            "error must name socket path: {err_text}"
        );
        assert!(
            err_text.contains("systemctl status autocoder"),
            "{err_text}"
        );
    }

    // ---- log parsing + rendering ----

    #[test]
    fn parse_stream_log_extracts_kinds() {
        let raw = "[tool_use] Read autocoder/src/foo.rs\n\
                   [tool_result] (123 bytes returned)\n\
                   [assistant] I'm thinking.\n";
        let events = parse_stream_log(raw);
        assert_eq!(events.len(), 3);
        match &events[0] {
            StreamEvent::ToolUse { name, input_summary, .. } => {
                assert_eq!(name, "Read");
                assert_eq!(input_summary, "autocoder/src/foo.rs");
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
        match &events[1] {
            StreamEvent::ToolResult { summary, .. } => {
                assert_eq!(summary, "(123 bytes returned)");
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
        match &events[2] {
            StreamEvent::Assistant { text, .. } => {
                assert_eq!(text, "I'm thinking.");
            }
            other => panic!("expected Assistant, got {other:?}"),
        }
    }

    #[test]
    fn parse_stream_log_recognizes_timestamps_when_present() {
        let raw = "[01:23:47.123] [tool_use] Read foo\n\
                   [01:23:47.401] [tool_result] (10 bytes returned)\n";
        let events = parse_stream_log(raw);
        assert_eq!(events[0].timestamp(), Some("01:23:47.123"));
        assert_eq!(events[1].timestamp(), Some("01:23:47.401"));
    }

    #[test]
    fn pair_tool_calls_groups_positional() {
        let raw = "[tool_use] Read a\n\
                   [tool_result] (1 bytes returned)\n\
                   [tool_use] Edit b\n\
                   [tool_result] (2 bytes returned)\n";
        let events = parse_stream_log(raw);
        let pairs = pair_tool_calls(&events);
        assert_eq!(pairs.len(), 2);
        for pair in &pairs {
            assert!(pair.tool_result.is_some());
        }
    }

    fn build_fixture_logs(
        paths: &DaemonPaths,
        basename: &str,
        change: &str,
        stream: &str,
        final_answer: &str,
    ) -> (PathBuf, PathBuf) {
        let runs = paths.run_logs_dir(basename);
        std::fs::create_dir_all(&runs).unwrap();
        let stream_path = runs.join(format!("{change}.stream.log"));
        let summary_path = runs.join(format!("{change}.log"));
        std::fs::write(&stream_path, stream).unwrap();
        let summary = format!(
            "=== PROMPT (1 bytes) ===\np\n=== ACTIONS (see {change}.stream.log) ===\n\
             \n=== FINAL ANSWER ({} bytes) ===\n{final_answer}\n\n=== STDERR (0 bytes) ===\n",
            final_answer.len(),
        );
        std::fs::write(&summary_path, summary).unwrap();
        (summary_path, stream_path)
    }

    #[tokio::test]
    async fn log_renders_fixture_stream() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = paths_with_workspaces(tmp.path(), &["github_com_foo_bar"]);
        let stream = "[tool_use] Read foo.rs\n\
                      [tool_result] (123 bytes returned)\n\
                      [tool_use] mcp__ask_user__query_canonical_specs {\"query\":\"outcome\",\"top_k\":5}\n\
                      [tool_result] (4321 bytes returned)\n";
        let (_summary, _stream_path) = build_fixture_logs(
            &paths,
            "github_com_foo_bar",
            "a30-baz",
            stream,
            "ALL DONE",
        );
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        log(
            &paths,
            LogArgs {
                workspace: Some("github_com_foo_bar".to_string()),
                change: "a30-baz".to_string(),
                limit: None,
                json: false,
            },
            &mut stdout,
            &mut stderr,
        )
        .await
        .unwrap();
        let s = String::from_utf8(stdout).unwrap();
        assert!(s.contains("== run log: a30-baz =="), "{s}");
        assert!(s.contains("workspace: github_com_foo_bar"), "{s}");
        assert!(s.contains("summary log:"), "{s}");
        assert!(s.contains("stream log:"), "{s}");
        assert!(s.contains("Read"), "{s}");
        assert!(s.contains("query_canonical_specs"), "{s}");
        assert!(s.contains("query=\"outcome\""), "{s}");
        assert!(s.contains("top_k=5"), "{s}");
        assert!(s.contains("=== FINAL ANSWER ==="), "{s}");
        assert!(s.contains("ALL DONE"), "{s}");
    }

    #[tokio::test]
    async fn log_limit_truncates_with_notice() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = paths_with_workspaces(tmp.path(), &["github_com_foo_bar"]);
        let mut stream_buf = String::new();
        for i in 0..10 {
            stream_buf.push_str(&format!(
                "[tool_use] Read file{i}.rs\n[tool_result] (1 bytes returned)\n"
            ));
        }
        build_fixture_logs(
            &paths,
            "github_com_foo_bar",
            "a30-baz",
            &stream_buf,
            "done",
        );
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        log(
            &paths,
            LogArgs {
                workspace: Some("github_com_foo_bar".to_string()),
                change: "a30-baz".to_string(),
                limit: Some(3),
                json: false,
            },
            &mut stdout,
            &mut stderr,
        )
        .await
        .unwrap();
        let s = String::from_utf8(stdout).unwrap();
        assert!(s.contains("truncated; 10 tool calls total"), "{s}");
    }

    #[tokio::test]
    async fn log_json_mode_emits_event_array() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = paths_with_workspaces(tmp.path(), &["github_com_foo_bar"]);
        build_fixture_logs(
            &paths,
            "github_com_foo_bar",
            "a30-baz",
            "[tool_use] Read foo.rs\n[tool_result] (1 bytes returned)\n",
            "done",
        );
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        log(
            &paths,
            LogArgs {
                workspace: Some("github_com_foo_bar".to_string()),
                change: "a30-baz".to_string(),
                limit: None,
                json: true,
            },
            &mut stdout,
            &mut stderr,
        )
        .await
        .unwrap();
        let s = String::from_utf8(stdout).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(s.trim()).unwrap();
        let arr = parsed.as_array().expect("JSON array");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["kind"], "tool_use");
        assert_eq!(arr[0]["name"], "Read");
    }

    #[test]
    fn missing_log_path_lists_available_changes_helper() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = paths_with_workspaces(tmp.path(), &["github_com_foo_bar"]);
        let runs = paths.run_logs_dir("github_com_foo_bar");
        std::fs::create_dir_all(&runs).unwrap();
        std::fs::write(runs.join("a01-existing.stream.log"), "").unwrap();
        std::fs::write(runs.join("a02-also-there.stream.log"), "").unwrap();
        std::fs::write(runs.join("a03-existing.log"), "").unwrap(); // not a stream
        let avail = list_available_changes(&runs);
        assert_eq!(avail, vec!["a01-existing", "a02-also-there"]);
    }

    #[tokio::test]
    async fn log_inner_missing_file_emits_available_changes_and_operator_error() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = paths_with_workspaces(tmp.path(), &["github_com_foo_bar"]);
        let runs = paths.run_logs_dir("github_com_foo_bar");
        std::fs::create_dir_all(&runs).unwrap();
        std::fs::write(runs.join("a01-existing.stream.log"), "").unwrap();
        std::fs::write(runs.join("a02-also-there.stream.log"), "").unwrap();
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let outcome = log_inner(
            &paths,
            LogArgs {
                workspace: Some("github_com_foo_bar".to_string()),
                change: "nonexistent-change".to_string(),
                limit: None,
                json: false,
            },
            &mut stdout,
            &mut stderr,
        )
        .await
        .unwrap();
        assert_eq!(outcome, InspectOutcome::OperatorError);
        let err = String::from_utf8(stderr).unwrap();
        assert!(err.contains("no stream log at"), "{err}");
        assert!(err.contains("nonexistent-change.stream.log"), "{err}");
        assert!(err.contains("Available changes"), "{err}");
        assert!(err.contains("a01-existing"), "{err}");
        assert!(err.contains("a02-also-there"), "{err}");
    }

    #[tokio::test]
    async fn tool_usage_inner_missing_file_emits_available_changes_and_operator_error() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = paths_with_workspaces(tmp.path(), &["github_com_foo_bar"]);
        let runs = paths.run_logs_dir("github_com_foo_bar");
        std::fs::create_dir_all(&runs).unwrap();
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let outcome = tool_usage_inner(
            &paths,
            ToolUsageArgs {
                workspace: Some("github_com_foo_bar".to_string()),
                change: "missing".to_string(),
                json: false,
            },
            &mut stdout,
            &mut stderr,
        )
        .await
        .unwrap();
        assert_eq!(outcome, InspectOutcome::OperatorError);
        let err = String::from_utf8(stderr).unwrap();
        assert!(err.contains("no stream log at"), "{err}");
        assert!(err.contains("(none)"), "no changes present, expected (none): {err}");
    }

    // ---- tool-usage stats ----

    #[test]
    fn aggregate_stats_counts_per_tool() {
        let raw = "[tool_use] Read a\n\
                   [tool_result] (1 bytes returned)\n\
                   [tool_use] Read b\n\
                   [tool_result] (2 bytes returned)\n\
                   [tool_use] Edit c\n\
                   [tool_result] (3 bytes returned)\n\
                   [tool_use] Bash ls\n\
                   [tool_result] (10 bytes returned)\n";
        let events = parse_stream_log(raw);
        let stats = aggregate_stats(&events);
        assert_eq!(*stats.tool_counts.get("Read").unwrap(), 2_usize);
        assert_eq!(*stats.tool_counts.get("Edit").unwrap(), 1_usize);
        assert_eq!(*stats.tool_counts.get("Bash").unwrap(), 1_usize);
        assert!(stats.query_canonical_specs.is_none());
    }

    #[test]
    fn aggregate_stats_rag_detail_from_structured_results() {
        // Forward-compat shape: when the tool_result line carries the
        // structured JSON (vs. just the byte summary), the rag detail
        // fields populate from it.
        let raw = "[tool_use] mcp__ask_user__query_canonical_specs {\"query\":\"x\",\"top_k\":5}\n\
                   [tool_result] {\"hits\":[{\"relevance_score\":0.9},{\"relevance_score\":0.6},{\"relevance_score\":0.4}]}\n";
        let events = parse_stream_log(raw);
        let stats = aggregate_stats(&events);
        let detail = stats.query_canonical_specs.expect("rag detail");
        assert_eq!(detail.calls, 1);
        assert_eq!(detail.total_hits, 3);
        assert_eq!(detail.high_score_hits, 1);
        assert_eq!(detail.medium_score_hits, 1);
        assert_eq!(detail.low_score_hits, 1);
        assert!((detail.avg_hits_per_call - 3.0).abs() < f64::EPSILON);
    }

    #[test]
    fn aggregate_stats_rag_detail_bytes_only_summary() {
        // Current daemon-on-disk shape: `(N bytes returned)`. Bytes
        // accumulate; hit counts cannot be recovered, so they stay zero.
        let raw = "[tool_use] mcp__ask_user__query_canonical_specs {\"query\":\"x\"}\n\
                   [tool_result] (18953 bytes returned)\n";
        let events = parse_stream_log(raw);
        let stats = aggregate_stats(&events);
        let detail = stats.query_canonical_specs.expect("rag detail present");
        assert_eq!(detail.calls, 1);
        assert!(detail.total_bytes_returned > 0);
        assert_eq!(detail.total_hits, 0);
    }

    #[tokio::test]
    async fn tool_usage_renders_against_fixture() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = paths_with_workspaces(tmp.path(), &["github_com_foo_bar"]);
        let stream = "[tool_use] Read a.rs\n\
                      [tool_result] (5 bytes returned)\n\
                      [tool_use] Edit b.rs\n\
                      [tool_result] (10 bytes returned)\n\
                      [tool_use] mcp__ask_user__query_canonical_specs {\"query\":\"x\",\"top_k\":3}\n\
                      [tool_result] {\"hits\":[{\"relevance_score\":0.85},{\"relevance_score\":0.55}]}\n";
        build_fixture_logs(
            &paths,
            "github_com_foo_bar",
            "a30-baz",
            stream,
            "done",
        );
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        tool_usage(
            &paths,
            ToolUsageArgs {
                workspace: Some("github_com_foo_bar".to_string()),
                change: "a30-baz".to_string(),
                json: false,
            },
            &mut stdout,
            &mut stderr,
        )
        .await
        .unwrap();
        let s = String::from_utf8(stdout).unwrap();
        assert!(s.contains("== tool-usage summary: a30-baz =="), "{s}");
        assert!(s.contains("tool calls:"), "{s}");
        assert!(s.contains("Read"), "{s}");
        assert!(s.contains("Edit"), "{s}");
        assert!(s.contains("query_canonical_specs"), "{s}");
        assert!(s.contains("query_canonical_specs detail:"), "{s}");
        assert!(s.contains("high (>=0.7):   1"), "{s}");
        assert!(s.contains("medium (.5-.7): 1"), "{s}");
        assert!(s.contains("low (<.5):      0"), "{s}");
        assert!(s.contains("avg hits/call: 2.0"), "{s}");
    }

    #[tokio::test]
    async fn tool_usage_json_mode_emits_structured_object() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = paths_with_workspaces(tmp.path(), &["github_com_foo_bar"]);
        build_fixture_logs(
            &paths,
            "github_com_foo_bar",
            "a30-baz",
            "[tool_use] Read a\n[tool_result] (1 bytes returned)\n",
            "done",
        );
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        tool_usage(
            &paths,
            ToolUsageArgs {
                workspace: Some("github_com_foo_bar".to_string()),
                change: "a30-baz".to_string(),
                json: true,
            },
            &mut stdout,
            &mut stderr,
        )
        .await
        .unwrap();
        let s = String::from_utf8(stdout).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(s.trim()).unwrap();
        assert_eq!(parsed["tool_counts"]["Read"], 1);
    }
}
