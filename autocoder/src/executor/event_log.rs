//! `StructuredLogWriter` — incrementally writes a per-change log file
//! with PROMPT / ACTIONS / FINAL ANSWER / STDERR sections.
//!
//! The streaming write strategy guarantees that on timeout-kill, every
//! event the child emitted before the kill is durably on disk:
//!
//!   - `write_prompt` opens the file, writes the prompt section header,
//!     and emits the `=== ACTIONS ===` header.
//!   - `append_action` writes one diagnostic line under ACTIONS per call
//!     (typed by `ActionKind`).
//!   - `set_final_answer` buffers the `result` event's text in memory.
//!   - `append_stderr` accumulates stderr bytes in memory until
//!     `finalize` flushes them under the STDERR section.
//!   - `finalize` writes the trailing `=== FINAL ANSWER ===` and
//!     `=== STDERR ===` sections with byte counts in their headers.
//!
//! Section headers carry byte counts so an operator scanning the file
//! can quickly judge "how much did the model say" without reading
//! everything. The ACTIONS header is fixed; section sizes for FINAL
//! ANSWER / STDERR are computed at `finalize` time when both sections
//! are fully known.

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

/// Incremental writer for the structured per-change log.
pub struct StructuredLogWriter {
    inner: Mutex<Inner>,
    #[allow(dead_code)]
    path: PathBuf,
}

struct Inner {
    file: std::fs::File,
    final_answer: Option<String>,
    stderr_buf: Vec<u8>,
    finalized: bool,
}

/// Open the log file at `path` (creating its parent directory if needed)
/// and return a writer ready to accept `write_prompt` followed by any
/// number of `append_action` / `append_stderr` calls and one
/// `set_final_answer` / `finalize` at the end.
pub fn open(path: &Path) -> Result<StructuredLogWriter> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!("creating log directory {}", parent.display())
        })?;
    }
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .with_context(|| format!("opening log file {}", path.display()))?;
    Ok(StructuredLogWriter {
        inner: Mutex::new(Inner {
            file,
            final_answer: None,
            stderr_buf: Vec::new(),
            finalized: false,
        }),
        path: path.to_path_buf(),
    })
}

impl StructuredLogWriter {
    /// Write the PROMPT section header + body + the ACTIONS section
    /// header. Call once, before any `append_action`.
    pub fn write_prompt(&self, prompt: &str) -> Result<()> {
        let mut guard = self.inner.lock().unwrap();
        let header = format!("=== PROMPT ({n} bytes) ===\n", n = prompt.len());
        guard.file.write_all(header.as_bytes())?;
        guard.file.write_all(prompt.as_bytes())?;
        // Trailing newline if the prompt didn't already end with one, so
        // the ACTIONS header sits on its own line.
        if !prompt.ends_with('\n') {
            guard.file.write_all(b"\n")?;
        }
        guard.file.write_all(b"\n=== ACTIONS ===\n")?;
        Ok(())
    }

    /// Append one formatted action line under the ACTIONS section.
    /// Format: `<kind-prefix> <content>\n`. Multi-line content is
    /// wrapped at the caller's discretion (e.g. ToolUse summarizes its
    /// input; long Assistant text is split into multiple calls by the
    /// dispatcher).
    pub fn append_action(&self, kind: ActionKind, content: &str) -> Result<()> {
        let mut guard = self.inner.lock().unwrap();
        let line = format!("{} {}\n", kind.prefix(), content);
        guard.file.write_all(line.as_bytes())?;
        Ok(())
    }

    /// Capture the `result` event's final text. Held in memory until
    /// `finalize` writes the FINAL ANSWER section at the end of the file.
    pub fn set_final_answer(&self, text: String) -> Result<()> {
        let mut guard = self.inner.lock().unwrap();
        guard.final_answer = Some(text);
        Ok(())
    }

    /// Accumulate stderr bytes (buffered in memory; flushed under the
    /// STDERR section at `finalize` time).
    pub fn append_stderr(&self, bytes: &[u8]) -> Result<()> {
        let mut guard = self.inner.lock().unwrap();
        guard.stderr_buf.extend_from_slice(bytes);
        Ok(())
    }

    /// Write the trailing FINAL ANSWER + STDERR sections and flush. Safe
    /// to call multiple times — idempotent after the first call.
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
        guard.file.write_all(final_header.as_bytes())?;
        guard.file.write_all(final_text.as_bytes())?;
        if !final_text.is_empty() && !final_text.ends_with('\n') {
            guard.file.write_all(b"\n")?;
        }
        let stderr_header = format!(
            "\n=== STDERR ({n} bytes) ===\n",
            n = stderr_bytes.len()
        );
        guard.file.write_all(stderr_header.as_bytes())?;
        guard.file.write_all(&stderr_bytes)?;
        if !stderr_bytes.is_empty() && !stderr_bytes.ends_with(b"\n") {
            guard.file.write_all(b"\n")?;
        }
        guard.file.flush()?;
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

    /// Path the writer is writing to.
    #[allow(dead_code)]
    pub fn path(&self) -> &Path {
        &self.path
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

        let body = std::fs::read_to_string(&path).unwrap();
        // Section ordering and headers.
        let prompt_idx = body.find("=== PROMPT (").expect("PROMPT header");
        let actions_idx = body.find("=== ACTIONS ===").expect("ACTIONS header");
        let final_idx = body
            .find("=== FINAL ANSWER (")
            .expect("FINAL ANSWER header");
        let stderr_idx = body.find("=== STDERR (").expect("STDERR header");
        assert!(prompt_idx < actions_idx);
        assert!(actions_idx < final_idx);
        assert!(final_idx < stderr_idx);

        // Content presence with correct prefixes.
        assert!(body.contains("PROMPT_CONTENT"));
        assert!(body.contains("[tool_use] Read autocoder/src/foo.rs"));
        assert!(body.contains("[tool_result] (123 bytes returned)"));
        assert!(body.contains("[assistant] I'm looking at the file."));
        assert!(body.contains("All done — implemented the change."));
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

        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("=== FINAL ANSWER (0 bytes) ==="));
        // The action that DID arrive is preserved.
        assert!(body.contains("[tool_use] Read foo"));
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
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("[unknown:future_kind] {\"foo\":1}"));
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
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("[raw] malformed line content"));
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
