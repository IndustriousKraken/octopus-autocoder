//! `StructuredLogWriter` — incrementally writes per-change log files.
//!
//! Per a20a2, the output is split into TWO sibling files per change:
//!
//!   - **Summary log** at `<change>.log` containing the operator-facing
//!     content: PROMPT, an ACTIONS-pointer line naming the stream log,
//!     FINAL ANSWER, and STDERR sections.
//!   - **Stream log** at `<change>.stream.log` containing the verbose
//!     action stream (`[tool_use] ...`, `[tool_result] ...`,
//!     `[assistant] ...`, `[raw] ...`, `[unknown:<type>] ...` lines).
//!     No section headers; one continuous stream.
//!
//! The streaming write strategy guarantees that on timeout-kill, every
//! event the child emitted before the kill is durably on disk in the
//! stream log:
//!
//!   - `write_prompt` opens both files, writes the prompt section to
//!     the summary, emits the ACTIONS pointer line to the summary, AND
//!     creates the (initially empty) stream file.
//!   - `append_action` writes one diagnostic line to the stream log per
//!     call (typed by `ActionKind`).
//!   - `set_final_answer` buffers the `result` event's text in memory.
//!   - `append_stderr` accumulates stderr bytes in memory until
//!     `finalize` flushes them under the STDERR section of the summary.
//!   - `finalize` writes the trailing FINAL ANSWER + STDERR sections to
//!     the summary file.
//!
//! Splitting isolates the agent-controllable action stream from the
//! daemon-readable summary so consumers (sentinel scanner, PR-comment
//! composer, future scrapers) can't be tricked into matching against
//! tool-result echoes that happen to contain daemon-meaningful markers.
//! Operators reading the summary log see a short, signal-dense file
//! plus a pointer to where the verbose stream lives.

use anyhow::{Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Categorization of one action line. Determines the `[prefix]` written
/// before the content.
#[derive(Debug, Clone, PartialEq)]
pub enum ActionKind {
    ToolUse,
    ToolResult,
    Assistant,
    Raw,
    Unknown(String),
}

impl ActionKind {
    fn prefix(&self) -> String {
        match self {
            ActionKind::ToolUse => "[tool_use]".to_string(),
            ActionKind::ToolResult => "[tool_result]".to_string(),
            ActionKind::Assistant => "[assistant]".to_string(),
            ActionKind::Raw => "[raw]".to_string(),
            ActionKind::Unknown(t) => format!("[unknown:{t}]"),
        }
    }
}

/// Incremental writer for the structured per-change logs (summary +
/// stream).
pub struct StructuredLogWriter {
    inner: Mutex<Inner>,
    summary_path: PathBuf,
    stream_path: PathBuf,
}

struct Inner {
    summary_file: std::fs::File,
    stream_file: std::fs::File,
    final_answer: Option<String>,
    stderr_buf: Vec<u8>,
    finalized: bool,
    session_id: Option<String>,
}

/// Compute the stream log path for a given summary log path:
/// `<base>.log` → `<base>.stream.log`. Other extensions are appended
/// `.stream.log` as a safety fallback.
pub fn stream_path_for(summary_path: &Path) -> PathBuf {
    let stem = summary_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "stream".to_string());
    let stream_filename = format!("{stem}.stream.log");
    match summary_path.parent() {
        Some(parent) => parent.join(stream_filename),
        None => PathBuf::from(stream_filename),
    }
}

/// Open both per-change log files at the summary path AND its stream
/// sibling (creating the parent directory if needed) and return a writer
/// ready to accept `write_prompt` followed by any number of
/// `append_action` / `append_stderr` calls and one
/// `set_final_answer` / `finalize` at the end.
pub fn open(summary_path: &Path) -> Result<StructuredLogWriter> {
    if let Some(parent) = summary_path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!("creating log directory {}", parent.display())
        })?;
    }
    let summary_file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(summary_path)
        .with_context(|| format!("opening summary log file {}", summary_path.display()))?;
    let stream_path = stream_path_for(summary_path);
    let stream_file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&stream_path)
        .with_context(|| format!("opening stream log file {}", stream_path.display()))?;
    Ok(StructuredLogWriter {
        inner: Mutex::new(Inner {
            summary_file,
            stream_file,
            final_answer: None,
            stderr_buf: Vec::new(),
            finalized: false,
            session_id: None,
        }),
        summary_path: summary_path.to_path_buf(),
        stream_path,
    })
}

impl StructuredLogWriter {
    /// Write the PROMPT section header + body + the ACTIONS pointer
    /// line to the summary file. Call once, before any `append_action`.
    /// The stream file is already open (created by `open`); it remains
    /// empty until the first `append_action`.
    pub fn write_prompt(&self, prompt: &str) -> Result<()> {
        let mut guard = self.inner.lock().unwrap();
        let header = format!("=== PROMPT ({n} bytes) ===\n", n = prompt.len());
        guard.summary_file.write_all(header.as_bytes())?;
        guard.summary_file.write_all(prompt.as_bytes())?;
        // Trailing newline if the prompt didn't already end with one, so
        // the ACTIONS pointer line sits on its own line.
        if !prompt.ends_with('\n') {
            guard.summary_file.write_all(b"\n")?;
        }
        // Per a20a2: ACTIONS slot in summary is a single pointer line
        // naming the sibling stream-log file.
        let stream_filename = self
            .stream_path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "<change>.stream.log".to_string());
        let pointer = format!("\n=== ACTIONS (see {stream_filename}) ===\n");
        guard.summary_file.write_all(pointer.as_bytes())?;
        Ok(())
    }

    /// Append one formatted action line to the stream log file. Format:
    /// `<kind-prefix> <content>\n`. Multi-line content is wrapped at the
    /// caller's discretion (e.g. ToolUse summarizes its input; long
    /// Assistant text is split into multiple calls by the dispatcher).
    pub fn append_action(&self, kind: ActionKind, content: &str) -> Result<()> {
        let mut guard = self.inner.lock().unwrap();
        let line = format!("{} {}\n", kind.prefix(), content);
        guard.stream_file.write_all(line.as_bytes())?;
        Ok(())
    }

    /// Capture the `result` event's final text. Held in memory until
    /// `finalize` writes the FINAL ANSWER section at the end of the
    /// summary file.
    pub fn set_final_answer(&self, text: String) -> Result<()> {
        let mut guard = self.inner.lock().unwrap();
        guard.final_answer = Some(text);
        Ok(())
    }

    /// Accumulate stderr bytes (buffered in memory; flushed under the
    /// STDERR section of the summary file at `finalize` time).
    pub fn append_stderr(&self, bytes: &[u8]) -> Result<()> {
        let mut guard = self.inner.lock().unwrap();
        guard.stderr_buf.extend_from_slice(bytes);
        Ok(())
    }

    /// Write the trailing FINAL ANSWER + STDERR sections to the summary
    /// file AND flush both files. Safe to call multiple times —
    /// idempotent after the first call.
    pub fn finalize(&self) -> Result<()> {
        let mut guard = self.inner.lock().unwrap();
        if guard.finalized {
            return Ok(());
        }
        let final_text = guard.final_answer.clone().unwrap_or_default();
        let stderr_bytes = std::mem::take(&mut guard.stderr_buf);
        let final_header = format!(
            "\n=== FINAL ANSWER ({n} bytes) ===\n",
            n = final_text.len()
        );
        guard.summary_file.write_all(final_header.as_bytes())?;
        guard.summary_file.write_all(final_text.as_bytes())?;
        if !final_text.is_empty() && !final_text.ends_with('\n') {
            guard.summary_file.write_all(b"\n")?;
        }
        let stderr_header = format!(
            "\n=== STDERR ({n} bytes) ===\n",
            n = stderr_bytes.len()
        );
        guard.summary_file.write_all(stderr_header.as_bytes())?;
        guard.summary_file.write_all(&stderr_bytes)?;
        if !stderr_bytes.is_empty() && !stderr_bytes.ends_with(b"\n") {
            guard.summary_file.write_all(b"\n")?;
        }
        guard.summary_file.flush()?;
        guard.stream_file.flush()?;
        guard.finalized = true;
        Ok(())
    }

    /// In-memory final-answer text captured from the `result` event.
    /// Returns None when the run never reached the result event (timeout
    /// case).
    pub fn final_answer(&self) -> Option<String> {
        let guard = self.inner.lock().unwrap();
        guard.final_answer.clone()
    }

    /// Capture the session_id from a `system`-event `init` subtype. The
    /// recovery loop (a27a2) reads this so it can launch `claude
    /// --resume <session_id>` against the same conversation.
    pub fn set_session_id(&self, id: String) {
        let mut guard = self.inner.lock().unwrap();
        guard.session_id = Some(id);
    }

    /// In-memory session_id captured from the System event. Returns None
    /// when no system-init event was seen (text mode, malformed stream,
    /// timeout before init, etc.).
    pub fn session_id(&self) -> Option<String> {
        let guard = self.inner.lock().unwrap();
        guard.session_id.clone()
    }

    /// Path of the summary log file.
    #[allow(dead_code)]
    pub fn path(&self) -> &Path {
        &self.summary_path
    }

    /// Path of the stream log file (sibling to summary).
    #[allow(dead_code)]
    pub fn stream_path(&self) -> &Path {
        &self.stream_path
    }
}

/// Read the FINAL ANSWER section out of an already-written log file.
/// Returns the text between the `=== FINAL ANSWER (<n> bytes) ===`
/// header and the next `=== ` header (or EOF). Returns None when the
/// header is absent OR the section is empty.
pub fn read_final_answer(log_path: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(log_path).ok()?;
    extract_final_answer(&raw)
}

fn extract_final_answer(raw: &str) -> Option<String> {
    let marker = "=== FINAL ANSWER (";
    let header_idx = raw.find(marker)?;
    // Advance past the header line (everything up to and including the
    // newline that ends `=== FINAL ANSWER (n bytes) ===`).
    let after_header_rel = raw[header_idx..].find('\n')?;
    let body_start = header_idx + after_header_rel + 1;
    // Section ends at the next `\n=== ` marker (the leading newline
    // protects against the section's own header matching).
    let body_end = raw[body_start..]
        .find("\n=== ")
        .map(|rel| body_start + rel)
        .unwrap_or(raw.len());
    let body = raw[body_start..body_end].trim_end_matches('\n');
    if body.is_empty() {
        None
    } else {
        Some(body.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn log_path(dir: &TempDir, name: &str) -> PathBuf {
        dir.path().join(name)
    }

    #[test]
    fn write_prompt_then_actions_then_finalize_produces_all_sections() {
        let tmp = TempDir::new().unwrap();
        let path = log_path(&tmp, "run.log");
        let writer = open(&path).unwrap();
        writer.write_prompt("PROMPT_CONTENT").unwrap();
        writer
            .append_action(ActionKind::ToolUse, "Read autocoder/src/foo.rs")
            .unwrap();
        writer
            .append_action(ActionKind::ToolResult, "(123 bytes returned)")
            .unwrap();
        writer
            .append_action(ActionKind::Assistant, "I'm looking at the file.")
            .unwrap();
        writer
            .set_final_answer("All done — implemented the change.".to_string())
            .unwrap();
        writer.finalize().unwrap();

        // Summary log: PROMPT + ACTIONS pointer + FINAL ANSWER + STDERR.
        let summary = std::fs::read_to_string(&path).unwrap();
        let prompt_idx = summary.find("=== PROMPT (").expect("PROMPT header");
        let actions_idx = summary
            .find("=== ACTIONS (see ")
            .expect("ACTIONS pointer line");
        let final_idx = summary
            .find("=== FINAL ANSWER (")
            .expect("FINAL ANSWER header");
        let stderr_idx = summary.find("=== STDERR (").expect("STDERR header");
        assert!(prompt_idx < actions_idx);
        assert!(actions_idx < final_idx);
        assert!(final_idx < stderr_idx);

        // Summary content presence: PROMPT, FINAL ANSWER. NOT actions.
        assert!(summary.contains("PROMPT_CONTENT"));
        assert!(summary.contains("All done — implemented the change."));
        assert!(
            !summary.contains("[tool_use]"),
            "summary log must NOT contain action stream content"
        );
        assert!(
            !summary.contains("[tool_result]"),
            "summary log must NOT contain action stream content"
        );
        assert!(
            !summary.contains("[assistant]"),
            "summary log must NOT contain action stream content"
        );
        // Pointer line names the sibling stream file.
        assert!(summary.contains("=== ACTIONS (see run.stream.log) ==="));

        // Stream log: action lines only.
        let stream_path = path.with_extension("stream.log");
        let stream = std::fs::read_to_string(&stream_path).unwrap();
        assert!(stream.contains("[tool_use] Read autocoder/src/foo.rs"));
        assert!(stream.contains("[tool_result] (123 bytes returned)"));
        assert!(stream.contains("[assistant] I'm looking at the file."));
        // Stream log has NO section headers.
        assert!(!stream.contains("=== PROMPT"));
        assert!(!stream.contains("=== ACTIONS"));
        assert!(!stream.contains("=== FINAL ANSWER"));
        assert!(!stream.contains("=== STDERR"));
    }

    #[test]
    fn final_answer_returned_post_finalize() {
        let tmp = TempDir::new().unwrap();
        let writer = open(&log_path(&tmp, "run.log")).unwrap();
        writer.write_prompt("p").unwrap();
        writer.set_final_answer("the answer".to_string()).unwrap();
        writer.finalize().unwrap();
        assert_eq!(writer.final_answer().as_deref(), Some("the answer"));
    }

    #[test]
    fn timeout_case_writes_empty_final_answer_section() {
        let tmp = TempDir::new().unwrap();
        let path = log_path(&tmp, "run.log");
        let writer = open(&path).unwrap();
        writer.write_prompt("p").unwrap();
        writer
            .append_action(ActionKind::ToolUse, "Read foo")
            .unwrap();
        // No set_final_answer — simulates the timeout-kill case.
        writer.finalize().unwrap();
        assert!(writer.final_answer().is_none());

        // Summary log: empty FINAL ANSWER section; NO action content.
        let summary = std::fs::read_to_string(&path).unwrap();
        assert!(summary.contains("=== FINAL ANSWER (0 bytes) ==="));
        assert!(
            !summary.contains("[tool_use]"),
            "summary log must NOT carry action content"
        );

        // Stream log: the action that DID arrive is preserved there.
        let stream_path = path.with_extension("stream.log");
        let stream = std::fs::read_to_string(&stream_path).unwrap();
        assert!(stream.contains("[tool_use] Read foo"));
    }

    #[test]
    fn zero_action_run_creates_both_files_with_empty_stream() {
        // Per a20a2: zero-action runs still create both files for
        // diagnostic consistency. The stream file exists but is empty;
        // the summary log has the pointer line referencing it.
        let tmp = TempDir::new().unwrap();
        let path = log_path(&tmp, "run.log");
        let writer = open(&path).unwrap();
        writer.write_prompt("p").unwrap();
        writer
            .set_final_answer("Quick reply with no tool calls.".to_string())
            .unwrap();
        writer.finalize().unwrap();

        let stream_path = path.with_extension("stream.log");
        assert!(path.exists(), "summary log must exist");
        assert!(stream_path.exists(), "stream log must exist even when empty");

        let summary = std::fs::read_to_string(&path).unwrap();
        assert!(summary.contains("=== ACTIONS (see run.stream.log) ==="));
        assert!(summary.contains("Quick reply with no tool calls."));

        let stream = std::fs::read_to_string(&stream_path).unwrap();
        assert!(stream.is_empty(), "zero-action stream log must be empty");
    }

    #[test]
    fn stream_path_for_replaces_log_extension() {
        let p = PathBuf::from("/logs/runs/foo/my-change.log");
        let s = stream_path_for(&p);
        assert_eq!(s, PathBuf::from("/logs/runs/foo/my-change.stream.log"));
    }

    #[test]
    fn stream_path_for_handles_non_log_extension() {
        // Defensive: any other extension still gets `.stream.log`
        // appended via the file_stem fallback.
        let p = PathBuf::from("/logs/runs/foo/odd-name.txt");
        let s = stream_path_for(&p);
        assert_eq!(s, PathBuf::from("/logs/runs/foo/odd-name.stream.log"));
    }

    #[test]
    fn unknown_kind_uses_event_type_in_prefix() {
        let tmp = TempDir::new().unwrap();
        let path = log_path(&tmp, "run.log");
        let writer = open(&path).unwrap();
        writer.write_prompt("p").unwrap();
        writer
            .append_action(
                ActionKind::Unknown("future_kind".to_string()),
                "{\"foo\":1}",
            )
            .unwrap();
        writer.finalize().unwrap();
        let stream = std::fs::read_to_string(path.with_extension("stream.log")).unwrap();
        assert!(stream.contains("[unknown:future_kind] {\"foo\":1}"));
    }

    #[test]
    fn raw_kind_uses_raw_prefix() {
        let tmp = TempDir::new().unwrap();
        let path = log_path(&tmp, "run.log");
        let writer = open(&path).unwrap();
        writer.write_prompt("p").unwrap();
        writer
            .append_action(ActionKind::Raw, "malformed line content")
            .unwrap();
        writer.finalize().unwrap();
        let stream = std::fs::read_to_string(path.with_extension("stream.log")).unwrap();
        assert!(stream.contains("[raw] malformed line content"));
    }

    #[test]
    fn stderr_bytes_land_in_stderr_section() {
        let tmp = TempDir::new().unwrap();
        let path = log_path(&tmp, "run.log");
        let writer = open(&path).unwrap();
        writer.write_prompt("p").unwrap();
        writer.append_stderr(b"first line\n").unwrap();
        writer.append_stderr(b"second line\n").unwrap();
        writer.finalize().unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("=== STDERR ("));
        assert!(body.contains("first line"));
        assert!(body.contains("second line"));
    }

    #[test]
    fn finalize_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let path = log_path(&tmp, "run.log");
        let writer = open(&path).unwrap();
        writer.write_prompt("p").unwrap();
        writer.finalize().unwrap();
        writer.finalize().unwrap();
        // Should still have exactly one FINAL ANSWER header.
        let body = std::fs::read_to_string(&path).unwrap();
        let count = body.matches("=== FINAL ANSWER (").count();
        assert_eq!(count, 1, "finalize must not double-write sections");
    }

    #[test]
    fn read_final_answer_extracts_section() {
        let tmp = TempDir::new().unwrap();
        let path = log_path(&tmp, "run.log");
        let writer = open(&path).unwrap();
        writer.write_prompt("p").unwrap();
        writer.set_final_answer("the conversational summary".to_string()).unwrap();
        writer.finalize().unwrap();

        let read = read_final_answer(&path).unwrap();
        assert_eq!(read, "the conversational summary");
    }

    #[test]
    fn read_final_answer_returns_none_when_section_empty() {
        let tmp = TempDir::new().unwrap();
        let path = log_path(&tmp, "run.log");
        let writer = open(&path).unwrap();
        writer.write_prompt("p").unwrap();
        // No set_final_answer.
        writer.finalize().unwrap();
        assert!(read_final_answer(&path).is_none());
    }

    #[test]
    fn read_final_answer_returns_none_when_section_missing() {
        // Legacy log file (text-mode opt-out shape) without a FINAL
        // ANSWER section at all.
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("legacy.log");
        std::fs::write(
            &path,
            "=== PROMPT (3 bytes) ===\nabc\n=== STDOUT (5 bytes) ===\nhello\n=== STDERR (0 bytes) ===\n",
        )
        .unwrap();
        assert!(read_final_answer(&path).is_none());
    }

    #[test]
    fn read_final_answer_handles_multi_paragraph_content() {
        let tmp = TempDir::new().unwrap();
        let path = log_path(&tmp, "run.log");
        let writer = open(&path).unwrap();
        writer.write_prompt("p").unwrap();
        writer
            .set_final_answer("first paragraph\n\nsecond paragraph".to_string())
            .unwrap();
        writer.finalize().unwrap();

        let read = read_final_answer(&path).unwrap();
        assert!(read.contains("first paragraph"));
        assert!(read.contains("second paragraph"));
        // Must NOT bleed into the STDERR section.
        assert!(!read.contains("STDERR"));
    }
}
