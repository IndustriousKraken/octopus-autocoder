//! Change-internal contradiction pre-flight check (a19).
//!
//! `a17`'s archivability check catches structural defects (MODIFIED title
//! missing from canonical, ADDED title already present). It does NOT
//! catch semantic defects — a change whose requirements are individually
//! well-formed AND archivable but contradict each other (ADDED A says
//! "all secrets in env vars"; ADDED B says "the API key in
//! `config.yaml`"). Pure-text logic cannot reliably detect this;
//! contradictions hide in domain language across multiple SHALL clauses.
//!
//! This module runs a configurable LLM against the change's concatenated
//! spec-delta files AND parses the response as a list of contradictions.
//! Failures (network, parse, malformed JSON) FAIL OPEN — we log a WARN
//! and return an empty Vec, matching the conservative bias documented in
//! the proposal: a flaky pre-flight should not block work.

use crate::llm::LlmClient;
use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use std::future::Future;
use std::path::Path;
use std::sync::Arc;

/// Runtime context for the contradiction-check pre-flight.
///
/// `llm` is the configured `LlmClient` (Anthropic or OpenAI-compatible
/// per the operator's `executor.change_internal_contradiction_check_llm`
/// block). `prompt_template` is the resolved prompt body — either the
/// embedded default OR the override file's contents.
///
/// Constructed once at daemon startup when the check is enabled. The
/// polling loop reads it on every iteration via [`current`].
pub struct ContradictionCheckCtx {
    pub llm: Arc<dyn LlmClient>,
    pub prompt_template: String,
}

tokio::task_local! {
    /// Per-task contradiction-check context. Set ONCE by [`scope`] at
    /// the top of the polling-task future; the polling loop reads it
    /// at each per-change pre-flight via [`current`]. Tests that do
    /// not call `scope` see `None`, so the global-state pollution
    /// problem from `OnceLock`-based designs does not apply.
    static CTX: Option<Arc<ContradictionCheckCtx>>;
}

/// Run `fut` with the given contradiction-check context bound for the
/// duration of the future. `None` represents the disabled state; the
/// polling loop's [`current`] reader returns `None` AND the check is a
/// no-op. Production callers (one per polling task) wrap the top-level
/// future once at startup.
pub fn scope<F>(ctx: Option<Arc<ContradictionCheckCtx>>, fut: F) -> impl Future<Output = F::Output>
where
    F: Future,
{
    CTX.scope(ctx, fut)
}

/// Snapshot of the current task's context. `None` when the operator
/// did not opt in OR the surrounding task did not call [`scope`].
/// Cheap clone of an `Arc`.
pub fn current() -> Option<Arc<ContradictionCheckCtx>> {
    CTX.try_with(|c| c.clone()).ok().flatten()
}

/// Default prompt template embedded at compile time. Overridable via
/// `executor.change_internal_contradiction_check_prompt_path`.
pub const EMBEDDED_PROMPT: &str =
    include_str!("../../../prompts/change-contradiction-check.md");

/// Resolve the prompt template. `None` returns the embedded default.
/// `Some(path)` reads the override file; an empty file (after `trim`) is
/// an error so the daemon does NOT feed an empty prompt to the LLM.
pub fn load_prompt_template(override_path: Option<&Path>) -> Result<String> {
    match override_path {
        None => Ok(EMBEDDED_PROMPT.to_string()),
        Some(path) => {
            let body = std::fs::read_to_string(path).with_context(|| {
                format!(
                    "reading change-contradiction-check prompt override at {}",
                    path.display()
                )
            })?;
            if body.trim().is_empty() {
                return Err(anyhow!(
                    "change-contradiction-check prompt override at {} is empty; refusing to feed an empty prompt to the LLM",
                    path.display()
                ));
            }
            Ok(body)
        }
    }
}

/// One contradiction surfaced by [`check_change_internal_contradictions`].
/// Mirrors the LLM's JSON output shape one-for-one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContradictionFinding {
    pub requirement_a: String,
    pub requirement_b: String,
    pub summary: String,
}

#[derive(Debug, Deserialize)]
struct LlmResponse {
    contradictions: Vec<LlmContradiction>,
}

#[derive(Debug, Deserialize)]
struct LlmContradiction {
    requirement_a: String,
    requirement_b: String,
    summary: String,
}

const PROMPT_DELIMITER: &str = "\n\n---\n\n";
const RESPONSE_EXCERPT_MAX: usize = 200;

/// Run the contradiction check for `change_slug` under `workspace_root`.
///
/// Reads every `<workspace>/openspec/changes/<change>/specs/<cap>/spec.md`,
/// concatenates them under `## File:` headers (same convention the
/// reviewer uses), appends them to the prompt template, invokes
/// `llm.complete(prompt)`, AND parses the response as
/// `{ contradictions: [...] }`.
///
/// Returns an empty `Vec` on every fail-open path: no spec deltas to
/// check, LLM transport error, malformed response. WARN logs name the
/// specific failure so operators can investigate via journalctl.
pub async fn check_change_internal_contradictions(
    workspace_root: &Path,
    repo: &crate::config::RepositoryConfig,
    change_slug: &str,
    llm: &dyn LlmClient,
    prompt_template: &str,
) -> Result<Vec<ContradictionFinding>> {
    let input = build_spec_input(workspace_root, repo, change_slug)?;
    let prompt = format!(
        "{template}{delim}{input}",
        template = prompt_template.trim_end(),
        delim = PROMPT_DELIMITER,
        input = input,
    );
    let response = match llm.complete(&prompt).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(
                change = %change_slug,
                "change-contradiction-check LLM call failed; treating as no contradictions found (fail-open): {e:#}"
            );
            return Ok(Vec::new());
        }
    };
    match parse_llm_response(&response) {
        Ok(findings) => Ok(findings),
        Err(e) => {
            let excerpt: String =
                response.chars().take(RESPONSE_EXCERPT_MAX).collect();
            tracing::warn!(
                change = %change_slug,
                "change-contradiction-check response did not parse as expected JSON; treating as no contradictions found (fail-open): {e:#}. Response excerpt: {excerpt}"
            );
            Ok(Vec::new())
        }
    }
}

/// Concatenate every `<workspace>/openspec/changes/<change>/specs/<cap>/spec.md`
/// into a single input string. Each file is prefixed by
/// `## File: openspec/changes/<change>/specs/<cap>/spec.md` so the LLM
/// can name the source when reporting a finding. Returns the empty
/// string when the change has no `specs/` subdir or no per-capability
/// spec files — the caller passes that through to the LLM, which
/// reports `{"contradictions": []}` on an empty input.
fn build_spec_input(
    workspace_root: &Path,
    repo: &crate::config::RepositoryConfig,
    change_slug: &str,
) -> Result<String> {
    // a26: route the change-dir lookup via SpecRoot so external
    // spec_storage is consulted when configured.
    let specs_dir = crate::workspace::spec_root::changes_dir(repo, workspace_root)
        .join(change_slug)
        .join("specs");
    if !specs_dir.is_dir() {
        return Ok(String::new());
    }
    let read = std::fs::read_dir(&specs_dir)
        .with_context(|| format!("reading {}", specs_dir.display()))?;
    let mut caps: Vec<(String, std::path::PathBuf)> = Vec::new();
    for entry in read.flatten() {
        let name = match entry.file_name().into_string() {
            Ok(s) => s,
            Err(_) => continue,
        };
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        caps.push((name, path));
    }
    caps.sort_by(|a, b| a.0.cmp(&b.0));

    let mut out = String::new();
    for (cap_name, cap_path) in caps {
        let spec_md = cap_path.join("spec.md");
        if !spec_md.is_file() {
            continue;
        }
        let body = match std::fs::read_to_string(&spec_md) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    capability = %cap_name,
                    "change-contradiction-check: cannot read {}: {e}; skipping",
                    spec_md.display()
                );
                continue;
            }
        };
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!(
            "## File: openspec/changes/{change}/specs/{cap}/spec.md\n\n",
            change = change_slug,
            cap = cap_name,
        ));
        out.push_str(&body);
        if !body.ends_with('\n') {
            out.push('\n');
        }
    }
    Ok(out)
}

/// Extract `contradictions: [...]` from the LLM's response. Accepts EITHER a
/// bare JSON object OR a JSON object wrapped in a Markdown code fence
/// (```json ... ``` or ``` ... ```) — both are common LLM output shapes.
fn parse_llm_response(raw: &str) -> Result<Vec<ContradictionFinding>> {
    let candidate = extract_json_object(raw)?;
    let parsed: LlmResponse = serde_json::from_str(candidate)
        .context("deserializing LLM response as {\"contradictions\": [...]}")?;
    Ok(parsed
        .contradictions
        .into_iter()
        .map(|c| ContradictionFinding {
            requirement_a: c.requirement_a,
            requirement_b: c.requirement_b,
            summary: c.summary,
        })
        .collect())
}

/// Locate the JSON object inside the LLM response. Strips a leading
/// Markdown code fence if present, then trims to the first balanced
/// `{...}` so trailing prose doesn't confuse the parser.
fn extract_json_object(raw: &str) -> Result<&str> {
    let trimmed = raw.trim();
    // Strip Markdown code fence if present.
    let fenced = if let Some(rest) = trimmed.strip_prefix("```json") {
        rest.trim_start_matches('\n')
    } else if let Some(rest) = trimmed.strip_prefix("```") {
        rest.trim_start_matches('\n')
    } else {
        trimmed
    };
    let fenced = fenced
        .trim_end()
        .strip_suffix("```")
        .map(str::trim_end)
        .unwrap_or(fenced);
    // Find the first `{` and the matching `}` (balance-aware).
    let bytes = fenced.as_bytes();
    let start = bytes
        .iter()
        .position(|b| *b == b'{')
        .ok_or_else(|| anyhow::anyhow!("response contains no JSON object"))?;
    let mut depth: i32 = 0;
    let mut end: Option<usize> = None;
    let mut in_str = false;
    let mut escape = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if escape {
            escape = false;
            continue;
        }
        if in_str {
            match b {
                b'\\' => escape = true,
                b'"' => in_str = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(i + 1);
                    break;
                }
            }
            _ => {}
        }
    }
    let end = end.ok_or_else(|| anyhow::anyhow!("response JSON object is unbalanced"))?;
    Ok(&fenced[start..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Mutex;
    use tempfile::TempDir;

    fn cc_fixture_repo() -> crate::config::RepositoryConfig {
        crate::config::RepositoryConfig {
            url: "git@github.com:owner/repo.git".to_string(),
            local_path: None,
            base_branch: "main".to_string(),
            agent_branch: "agent-q".to_string(),
            poll_interval_sec: 60,
            chatops_channel_id: None,
            max_changes_per_pr: None,
            audits: None,
            spec_storage: None,
            upstream: None,
            auto_submit_pr: true,
        }
    }

    struct FixedResponseLlm {
        body: Mutex<Option<String>>,
        error: Mutex<Option<String>>,
        last_prompt: Mutex<Option<String>>,
    }

    impl FixedResponseLlm {
        fn ok(body: &str) -> Self {
            Self {
                body: Mutex::new(Some(body.to_string())),
                error: Mutex::new(None),
                last_prompt: Mutex::new(None),
            }
        }
        fn err(msg: &str) -> Self {
            Self {
                body: Mutex::new(None),
                error: Mutex::new(Some(msg.to_string())),
                last_prompt: Mutex::new(None),
            }
        }
        fn last_prompt(&self) -> String {
            self.last_prompt
                .lock()
                .unwrap()
                .clone()
                .expect("complete() was never called")
        }
    }

    #[async_trait]
    impl LlmClient for FixedResponseLlm {
        async fn complete(&self, prompt: &str) -> Result<String> {
            *self.last_prompt.lock().unwrap() = Some(prompt.to_string());
            if let Some(msg) = self.error.lock().unwrap().clone() {
                return Err(anyhow::anyhow!(msg));
            }
            Ok(self.body.lock().unwrap().clone().unwrap_or_default())
        }
    }

    fn write(p: &Path, body: &str) {
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, body).unwrap();
    }

    fn write_change_spec(workspace: &Path, change: &str, capability: &str, body: &str) {
        write(
            &workspace
                .join("openspec/changes")
                .join(change)
                .join("specs")
                .join(capability)
                .join("spec.md"),
            body,
        );
    }

    #[tokio::test]
    async fn empty_contradictions_array_returns_empty_vec() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        write_change_spec(
            ws,
            "c1",
            "cap",
            "## ADDED Requirements\n\n### Requirement: New\nThe system SHALL new.\n",
        );
        let llm = FixedResponseLlm::ok(r#"{"contradictions": []}"#);
        let out = check_change_internal_contradictions(ws, &cc_fixture_repo(), "c1", &llm, "template")
            .await
            .unwrap();
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn single_contradiction_is_parsed() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        write_change_spec(
            ws,
            "c1",
            "cap",
            "## ADDED Requirements\n\n### Requirement: A\nThe system SHALL a.\n\n### Requirement: B\nThe system SHALL b.\n",
        );
        let body = r#"{
          "contradictions": [
            {
              "requirement_a": "A",
              "requirement_b": "B",
              "summary": "A and B cannot both hold"
            }
          ]
        }"#;
        let llm = FixedResponseLlm::ok(body);
        let out = check_change_internal_contradictions(ws, &cc_fixture_repo(), "c1", &llm, "template")
            .await
            .unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].requirement_a, "A");
        assert_eq!(out[0].requirement_b, "B");
        assert_eq!(out[0].summary, "A and B cannot both hold");
    }

    #[tokio::test]
    async fn malformed_json_fails_open_with_empty_vec() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        write_change_spec(
            ws,
            "c1",
            "cap",
            "## ADDED Requirements\n\n### Requirement: A\nThe system SHALL a.\n",
        );
        let llm = FixedResponseLlm::ok("this is not json at all");
        let out = check_change_internal_contradictions(ws, &cc_fixture_repo(), "c1", &llm, "template")
            .await
            .unwrap();
        assert!(out.is_empty(), "malformed JSON must fail open: {out:?}");
    }

    #[tokio::test]
    async fn llm_transport_error_fails_open_with_empty_vec() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        write_change_spec(
            ws,
            "c1",
            "cap",
            "## ADDED Requirements\n\n### Requirement: A\nThe system SHALL a.\n",
        );
        let llm = FixedResponseLlm::err("simulated network error");
        let out = check_change_internal_contradictions(ws, &cc_fixture_repo(), "c1", &llm, "template")
            .await
            .unwrap();
        assert!(out.is_empty(), "transport error must fail open: {out:?}");
    }

    #[tokio::test]
    async fn fenced_json_response_is_parsed() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        write_change_spec(
            ws,
            "c1",
            "cap",
            "## ADDED Requirements\n\n### Requirement: A\nThe system SHALL a.\n",
        );
        let body = "Here's my answer:\n```json\n{\"contradictions\": [{\"requirement_a\":\"A\",\"requirement_b\":\"B\",\"summary\":\"x\"}]}\n```\n";
        let llm = FixedResponseLlm::ok(body);
        let out = check_change_internal_contradictions(ws, &cc_fixture_repo(), "c1", &llm, "template")
            .await
            .unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].requirement_a, "A");
    }

    #[tokio::test]
    async fn change_with_no_specs_dir_still_calls_llm_with_empty_input() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        std::fs::create_dir_all(ws.join("openspec/changes/c1")).unwrap();
        let llm = FixedResponseLlm::ok(r#"{"contradictions": []}"#);
        let out = check_change_internal_contradictions(ws, &cc_fixture_repo(), "c1", &llm, "template")
            .await
            .unwrap();
        assert!(out.is_empty());
        let p = llm.last_prompt();
        assert!(
            p.starts_with("template"),
            "prompt must begin with the template; got: {p}"
        );
    }

    #[test]
    fn embedded_prompt_template_is_non_empty() {
        assert!(!EMBEDDED_PROMPT.trim().is_empty(), "embedded template must not be empty");
        assert!(EMBEDDED_PROMPT.contains("contradictions"));
    }

    #[test]
    fn load_prompt_template_none_returns_embedded() {
        let body = load_prompt_template(None).unwrap();
        assert_eq!(body, EMBEDDED_PROMPT);
    }

    #[test]
    fn load_prompt_template_some_reads_override_file() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("custom.md");
        std::fs::write(&p, "CUSTOM_TEMPLATE_BODY").unwrap();
        let body = load_prompt_template(Some(&p)).unwrap();
        assert_eq!(body, "CUSTOM_TEMPLATE_BODY");
    }

    #[test]
    fn load_prompt_template_empty_override_is_rejected() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("empty.md");
        std::fs::write(&p, "   \n\n  ").unwrap();
        let err =
            load_prompt_template(Some(&p)).expect_err("empty override must be rejected");
        let msg = format!("{err:#}");
        assert!(msg.contains(p.display().to_string().as_str()));
        assert!(
            msg.contains("empty"),
            "error must name the empty condition; got: {msg}"
        );
    }

    #[test]
    fn load_prompt_template_missing_override_path_errors() {
        let p = Path::new("/nonexistent/path/to/template.md");
        let err = load_prompt_template(Some(p)).expect_err("missing path must error");
        let msg = format!("{err:#}");
        assert!(msg.contains("/nonexistent/path/to/template.md"));
    }

    #[tokio::test]
    async fn prompt_concatenates_every_capability_spec_under_file_header() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        write_change_spec(
            ws,
            "c1",
            "alpha",
            "## ADDED Requirements\n\n### Requirement: A1\nBody.\n",
        );
        write_change_spec(
            ws,
            "c1",
            "beta",
            "## ADDED Requirements\n\n### Requirement: B1\nBody.\n",
        );
        let llm = FixedResponseLlm::ok(r#"{"contradictions": []}"#);
        let _ = check_change_internal_contradictions(ws, &cc_fixture_repo(), "c1", &llm, "PROMPT_TEMPLATE")
            .await
            .unwrap();
        let p = llm.last_prompt();
        assert!(p.contains("PROMPT_TEMPLATE"));
        assert!(p.contains("## File: openspec/changes/c1/specs/alpha/spec.md"));
        assert!(p.contains("## File: openspec/changes/c1/specs/beta/spec.md"));
        assert!(p.contains("Requirement: A1"));
        assert!(p.contains("Requirement: B1"));
    }
}
