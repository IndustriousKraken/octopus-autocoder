//! Global-rules pre-flight check — the `[rules]` gate of the verifier framework
//! (global-rules-gate).
//!
//! The `[canon]` gate ([`crate::preflight::canon_contradiction`]) catches a
//! change that contradicts THIS project's canonical specs. It cannot catch a
//! change that breaks a portable, project-AGNOSTIC engineering rule the operator
//! holds across every project (no futile tautological tests, prefer composition,
//! no committed secrets, …). The `[rules]` gate closes that gap: it is the
//! corpus-parameterized SIBLING of the `[canon]` gate — the SAME read-only
//! agentic machinery ([`crate::preflight::corpus_check`]) — but the comparison
//! corpus is the GLOBAL RULE CORPUS instead of the project's canonical specs,
//! AND each finding names the violated rule (by its stable id) rather than a
//! canonical requirement.
//!
//! The session runs with `ORCH_MCP_ROLE = global_rules_check` AND the
//! `submit_rule_violations` MCP tool. The rule corpus is small enough to feed
//! INLINE into the prompt at small scale (the agent reads the change's deltas
//! via `Read`, but the rules are inlined directly — see [`load_rule_corpus`],
//! which is the RETRIEVAL SEAM for relevant-subset selection once the corpus
//! outgrows the context window).
//!
//! Fail-closed posture (gatekeepers-fail-closed standard): a session error, a
//! never-corrected schema rejection, OR a session that ends with no submission
//! HOLDS the change (an `Errored` outcome carrying the `[verifier:rules]`
//! label), never waved through as "no violations". An empty submission is a
//! clean pass.

use crate::agentic_run::ResolvedModel;
use crate::preflight::corpus_check::{
    CliCorpusCheckSessionRunner, CorpusCheckSession, CorpusCheckSessionRunner,
    run_corpus_check_with_runner,
};
use crate::verifier_gate::VerifierGate;
use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The MCP role AND submission routing key the global-rules check uses. The
/// per-execution MCP child advertises `submit_rule_violations` ONLY when
/// `ORCH_MCP_ROLE` equals this value; the daemon-side schema validator is
/// registered under the same key (a56).
pub const GLOBAL_RULES_CHECK_ROLE: &str = "global_rules_check";

/// Default prompt template embedded at compile time. Overridable via
/// `executor.global_rules_check_prompt_path`.
pub const EMBEDDED_PROMPT: &str = include_str!("../../../prompts/global-rules-check.md");

const PROMPT_DELIMITER: &str = "\n\n---\n\n";

/// Rule-file extensions the corpus loader recognizes. A flat or grouped corpus
/// of markdown/text rule files; anything else is ignored.
const RULE_FILE_EXTENSIONS: &[&str] = &["md", "txt"];

/// The full `--allowedTools` list the global-rules-check sandbox grants: the
/// read-only file tools PLUS the qualified `submit_rule_violations` MCP tool.
/// Delegates to the shared corpus-check core. Exposed so tests can assert the
/// surface.
pub fn agentic_global_rules_allowed_tools() -> Vec<String> {
    crate::preflight::corpus_check::allowed_tools_for_role(GLOBAL_RULES_CHECK_ROLE)
}

/// Resolve the prompt template. `None` returns the embedded default;
/// `Some(path)` reads the override file (an empty file is rejected). Mirrors the
/// `[canon]` gate's loader.
pub fn load_prompt_template(override_path: Option<&Path>) -> Result<String> {
    match override_path {
        None => Ok(EMBEDDED_PROMPT.to_string()),
        Some(path) => {
            let body = std::fs::read_to_string(path).with_context(|| {
                format!("reading global-rules-check prompt override at {}", path.display())
            })?;
            if body.trim().is_empty() {
                return Err(anyhow!(
                    "global-rules-check prompt override at {} is empty; refusing to feed an empty prompt to the session",
                    path.display()
                ));
            }
            Ok(body)
        }
    }
}

/// One rule loaded from the corpus: its stable `id` (the file path relative to
/// the corpus root, without extension) AND its prose `body` (the one-sentence
/// rule + optional intent). A violation names the rule by `id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub id: String,
    pub body: String,
}

/// Load every rule from the corpus directory, supporting a FLAT layout (rule
/// files directly under the root) OR rules GROUPED into register subdirectories
/// (one level deep). The stable id is the file path relative to the corpus root
/// without its extension (e.g. `no-tautological-tests` or
/// `testing/no-tautological-tests`). `README.md` AND hidden files are skipped.
///
/// This is the RETRIEVAL SEAM (task 2.3): at small scale it returns ALL rules
/// (which the gate feeds inline to its session). When the corpus outgrows the
/// context window a relevant-subset selector replaces the "load all" body
/// WITHOUT changing the [`Rule`] shape or the call site.
pub fn load_rule_corpus(corpus_dir: &Path) -> Vec<Rule> {
    let mut rules = Vec::new();
    collect_rules(corpus_dir, corpus_dir, 0, &mut rules);
    rules.sort_by(|a, b| a.id.cmp(&b.id));
    rules
}

/// Recurse the corpus up to one level of register subdirectories, collecting
/// rule files. `depth` 0 is the corpus root; depth 1 is a register subdir; we do
/// not descend further (the protocol's only grouping axis is one level of
/// registers).
fn collect_rules(root: &Path, dir: &Path, depth: usize, out: &mut Vec<Rule>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with('.') {
            continue; // hidden files / dirs
        }
        let path = entry.path();
        if path.is_dir() {
            if depth == 0 {
                collect_rules(root, &path, depth + 1, out);
            }
            continue;
        }
        let ext_ok = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| RULE_FILE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
            .unwrap_or(false);
        if !ext_ok {
            continue;
        }
        if name.eq_ignore_ascii_case("README.md") {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        if body.trim().is_empty() {
            continue;
        }
        // Stable id: path relative to the corpus root, without extension, with
        // `/` separators (so a register-grouped rule is `register/rule-id`).
        let rel = path.strip_prefix(root).unwrap_or(&path);
        let rel_no_ext = rel.with_extension("");
        let id = rel_no_ext.to_string_lossy().replace('\\', "/");
        out.push(Rule {
            id,
            body: body.trim().to_string(),
        });
    }
}

/// Detect whether a configured corpus location is a git repo URL (vs. a local
/// path). A URL the daemon clones; a local path it reads in place. Exposed so
/// config startup-validation classifies a corpus the SAME way the resolver does
/// (one heuristic, not two that can drift apart).
pub fn is_git_url(corpus: &str) -> bool {
    corpus.starts_with("http://")
        || corpus.starts_with("https://")
        || corpus.starts_with("git@")
        || corpus.starts_with("ssh://")
        || corpus.ends_with(".git")
}

/// Sanitize a git URL into a stable directory name for the clone cache.
fn corpus_clone_name(url: &str) -> String {
    let trimmed = url.trim_end_matches('/').trim_end_matches(".git");
    let base = trimmed.rsplit(['/', ':']).next().unwrap_or("corpus");
    let cleaned: String = base
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    if cleaned.is_empty() {
        "corpus".to_string()
    } else {
        cleaned
    }
}

/// Resolve the configured corpus location into a local directory the daemon can
/// read. A local path is validated to exist as a directory. A git repo URL is
/// cloned into `cache_dir` (reused on subsequent startups). Returns the resolved
/// directory OR a fail-fast error (the caller turns this into a daemon-startup
/// failure so the misconfig surfaces before polling).
pub fn resolve_corpus(corpus: &str, cache_dir: &Path) -> Result<PathBuf> {
    let corpus = corpus.trim();
    if corpus.is_empty() {
        return Err(anyhow!("executor.global_rules.corpus is empty"));
    }
    if is_git_url(corpus) {
        let target = cache_dir.join(corpus_clone_name(corpus));
        if target.is_dir() {
            // Reuse a prior clone (best-effort freshness; an operator can clear
            // the cache dir to force a fresh clone).
            tracing::info!(
                corpus = %corpus,
                target = %target.display(),
                "global rule corpus: reusing existing clone"
            );
            return Ok(target);
        }
        std::fs::create_dir_all(cache_dir)
            .with_context(|| format!("creating global-rules corpus cache dir {}", cache_dir.display()))?;
        crate::git::clone(&target, corpus)
            .with_context(|| format!("cloning global rule corpus from {corpus}"))?;
        if !target.is_dir() {
            return Err(anyhow!(
                "global rule corpus clone of {corpus} did not produce a directory at {}",
                target.display()
            ));
        }
        Ok(target)
    } else {
        let path = PathBuf::from(corpus);
        if !path.exists() {
            return Err(anyhow!(
                "executor.global_rules.corpus path does not exist: {}",
                path.display()
            ));
        }
        if !path.is_dir() {
            return Err(anyhow!(
                "executor.global_rules.corpus must be a directory: {}",
                path.display()
            ));
        }
        Ok(path)
    }
}

/// Runtime context for the `[rules]` gate pre-flight. Parallel to the `[canon]`
/// gate's `CanonContradictionCheckCtx`, plus `corpus_dir` (the resolved local
/// directory holding the global rule corpus). Constructed once at daemon startup
/// when the check is enabled; the polling loop reads it per-iteration via
/// [`current`].
pub struct GlobalRulesCheckCtx {
    /// Wrapped CLI binary the agentic session spawns (`executor.command`).
    pub command: String,
    /// Resolved `(provider, model, api_base_url, api_key)` tuple (a56).
    pub model: ResolvedModel,
    /// Resolved prompt body (embedded default OR override file contents).
    pub prompt_template: String,
    /// Redaction-safe `<provider>/<model>` attribution surfaced on the
    /// operator-facing findings alert. `None` only for test contexts.
    pub attribution: Option<String>,
    /// Bounded retry of the agentic session on a no-submission outcome
    /// (`executor.verifier_gate_retries`).
    pub retries: u32,
    /// The resolved local directory holding the global rule corpus (a path, or a
    /// clone of the configured git repo). Read at prompt-build time.
    pub corpus_dir: PathBuf,
    /// Test-only injected `submit_rule_violations` submission, bypassing the CLI
    /// subprocess AND the control socket. `Some(Some(p))` stands in for a
    /// recorded payload; `Some(None)` simulates "agent never submitted"; `None`
    /// (default/production) uses the real CLI + `consume_submission` path.
    #[cfg(test)]
    pub test_submission: Option<Option<serde_json::Value>>,
}

tokio::task_local! {
    /// Per-task `[rules]` gate context. Set ONCE by [`scope`] at the top of the
    /// polling-task future; the polling loop reads it via [`current`]. `None`
    /// represents the disabled state.
    static CTX: Option<Arc<GlobalRulesCheckCtx>>;
}

/// Run `fut` with the given `[rules]` gate context bound for its duration.
/// `None` represents the disabled state; [`current`] then returns `None` AND the
/// gate is a no-op (its ledger runner records `Disabled`).
pub fn scope<F>(ctx: Option<Arc<GlobalRulesCheckCtx>>, fut: F) -> impl Future<Output = F::Output>
where
    F: Future,
{
    CTX.scope(ctx, fut)
}

/// Snapshot of the current task's `[rules]` gate context. `None` when the
/// operator did not opt in OR the surrounding task did not call [`scope`].
pub fn current() -> Option<Arc<GlobalRulesCheckCtx>> {
    CTX.try_with(|c| c.clone()).ok().flatten()
}

/// One global-rule violation surfaced by [`run_agentic_global_rules_check`].
/// Mirrors the `submit_rule_violations` payload's entry shape one-for-one: the
/// violated rule named by its stable `rule_id`, AND a one-line `summary` of how
/// the change violates it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleViolationFinding {
    pub rule_id: String,
    pub summary: String,
}

/// One entry as it arrives in the `submit_rule_violations` payload.
#[derive(Debug, Deserialize)]
struct RawRuleViolation {
    rule_id: String,
    summary: String,
}

/// The `submit_rule_violations` payload shape.
#[derive(Debug, Deserialize)]
struct RawRuleViolationSubmission {
    violations: Vec<RawRuleViolation>,
}

/// Validate AND map a consumed `submit_rule_violations` payload into
/// [`RuleViolationFinding`]s. This is BOTH the daemon-side schema validator
/// (registered via [`register_rule_violations_submission_schema`] with its `Ok`
/// value discarded) AND the consume-time mapper — so a payload that records
/// successfully is exactly one that maps (mirrors the `[canon]` gate's
/// `payload_to_canon_contradictions`).
pub(crate) fn payload_to_rule_violations(
    payload: &serde_json::Value,
) -> std::result::Result<Vec<RuleViolationFinding>, String> {
    let sub: RawRuleViolationSubmission = serde_json::from_value(payload.clone()).map_err(|e| {
        format!(
            "submit_rule_violations: payload does not match the expected shape \
             {{ violations: [{{ rule_id, summary }}] }}: {e}"
        )
    })?;
    Ok(sub
        .violations
        .into_iter()
        .map(|v| RuleViolationFinding {
            rule_id: v.rule_id,
            summary: v.summary,
        })
        .collect())
}

/// Register the `[rules]` gate's `submit_rule_violations` payload schema with
/// the daemon's submission store, under [`GLOBAL_RULES_CHECK_ROLE`]. Called once
/// at daemon startup alongside the other gates' schema registration.
pub fn register_rule_violations_submission_schema(
    store: &crate::submission_store::SubmissionStore,
) {
    store.register_schema(
        GLOBAL_RULES_CHECK_ROLE,
        Arc::new(|p: &serde_json::Value| payload_to_rule_violations(p).map(|_| ())),
    );
}

/// Outcome of the `[rules]` gate. Fails CLOSED (gatekeepers-fail-closed
/// standard): an inability to run is `Errored`, NEVER `Clean`. Mirrors
/// [`crate::preflight::canon_contradiction::CanonContradictionCheckOutcome`].
#[derive(Debug)]
pub enum GlobalRulesCheckOutcome {
    /// Ran successfully; no violations. Proceed.
    Clean,
    /// Ran successfully; found rule violations. Block (needs revision).
    Found(Vec<RuleViolationFinding>),
    /// Could NOT run (CLI unavailable, session error, no submission, or a re-map
    /// failure). Hold the change — never treat as `Clean`.
    Errored { cause: String },
}

/// Run the global-rules check for `change_slug` under `workspace_root`.
/// Production entry point invoked from the polling loop's pre-flight. Resolves
/// the CLI strategy from the model's provider (a56); a provider whose CLI has no
/// registered strategy FAILS CLOSED here with a WARN AND no subprocess is
/// spawned. Otherwise runs one read-only agentic session, drains the
/// `submit_rule_violations` submission, AND maps it to findings.
pub async fn run_agentic_global_rules_check(
    ctx: &GlobalRulesCheckCtx,
    workspace_root: &Path,
    change_slug: &str,
) -> GlobalRulesCheckOutcome {
    // Test seam: an injected submission stands in for the CLI + control socket.
    #[cfg(test)]
    if let Some(injected) = &ctx.test_submission {
        let runner = CannedRuleViolationRunner {
            submission: injected.clone(),
        };
        return run_agentic_global_rules_check_with_runner(ctx, workspace_root, change_slug, &runner)
            .await;
    }

    let strategy = match crate::agentic_run::strategy_for_provider(
        ctx.model.provider,
        ctx.command.clone(),
        Vec::new(),
    ) {
        Ok(s) => s,
        Err(e) => {
            let label = VerifierGate::Rules.label();
            let cause = format!("CLI strategy unavailable: {e:#}");
            tracing::warn!(
                change = %change_slug,
                "{label} global-rules-check could not run ({cause}); holding the change (fail-closed)"
            );
            return GlobalRulesCheckOutcome::Errored { cause };
        }
    };
    let runner = CliCorpusCheckSessionRunner {
        workspace: workspace_root,
        role: GLOBAL_RULES_CHECK_ROLE,
        allowed_tools: agentic_global_rules_allowed_tools(),
        strategy: strategy.as_ref(),
        model: &ctx.model,
        settings_dir: None,
        timeout: crate::preflight::corpus_check::CORPUS_CHECK_TIMEOUT,
        subject_noun: "global-rules-check",
    };
    run_agentic_global_rules_check_with_runner(ctx, workspace_root, change_slug, &runner).await
}

/// Map a corpus-check session result into a [`GlobalRulesCheckOutcome`] (the
/// rule-violation finding shape). The shared core handles session/retry/
/// fail-closed; this only re-maps the submitted payload.
fn map_rules_session(session: CorpusCheckSession, change_slug: &str) -> GlobalRulesCheckOutcome {
    let label = VerifierGate::Rules.label();
    match session {
        CorpusCheckSession::Errored { cause } => GlobalRulesCheckOutcome::Errored { cause },
        CorpusCheckSession::Submitted(payload) => match payload_to_rule_violations(&payload) {
            Ok(findings) if findings.is_empty() => GlobalRulesCheckOutcome::Clean,
            Ok(findings) => GlobalRulesCheckOutcome::Found(findings),
            Err(e) => {
                let cause = format!("submission failed re-validation: {e}");
                tracing::warn!(
                    change = %change_slug,
                    "{label} global-rules-check could not run ({cause}); holding the change (fail-closed)"
                );
                GlobalRulesCheckOutcome::Errored { cause }
            }
        },
    }
}

/// Orchestration shared by production AND tests. Builds the prompt (which inlines
/// the rule corpus), runs one session via `runner` through the shared
/// corpus-check core, AND maps the result with the fail-closed policy.
async fn run_agentic_global_rules_check_with_runner(
    ctx: &GlobalRulesCheckCtx,
    workspace_root: &Path,
    change_slug: &str,
    runner: &dyn CorpusCheckSessionRunner,
) -> GlobalRulesCheckOutcome {
    let prompt =
        build_global_rules_prompt(&ctx.prompt_template, workspace_root, change_slug, &ctx.corpus_dir);
    let session =
        run_corpus_check_with_runner(VerifierGate::Rules, change_slug, ctx.retries, &prompt, runner)
            .await;
    map_rules_session(session, change_slug)
}

/// Build the session prompt: the resolved template body, the change's spec-delta
/// file PATHS (the agent reads them on demand via `Read`), AND the global rule
/// corpus INLINED (each rule's id + body), then the `submit_rule_violations`
/// instruction. The corpus is fed inline because it is small; [`load_rule_corpus`]
/// is the retrieval seam for relevant-subset selection at scale.
fn build_global_rules_prompt(
    template: &str,
    workspace_root: &Path,
    change_slug: &str,
    corpus_dir: &Path,
) -> String {
    let delta_paths = crate::preflight::corpus_check::change_spec_delta_paths(workspace_root, change_slug);
    let rules = load_rule_corpus(corpus_dir);
    let mut out = String::new();
    out.push_str(template.trim_end());
    out.push_str(PROMPT_DELIMITER);

    out.push_str("# This change's spec-delta files\n\n");
    if delta_paths.is_empty() {
        out.push_str(
            "(this change has no spec-delta files under \
             openspec/changes/<change>/specs/ — there is nothing to check)\n",
        );
    } else {
        out.push_str(
            "Read each of these files with the `Read` tool — they are the change's \
             requirements:\n\n",
        );
        for p in &delta_paths {
            out.push_str(&format!("- {p}\n"));
        }
    }

    out.push_str("\n# The global rule corpus\n\n");
    if rules.is_empty() {
        out.push_str(
            "(the global rule corpus is empty — there are no rules for this change to \
             violate; submit an empty `violations` array)\n",
        );
    } else {
        out.push_str(
            "Each rule below is identified by a stable `id` (name a violation by this id). \
             A rule is a one-sentence assertion plus an optional rationale; judge the change \
             against its MEANING:\n\n",
        );
        for r in &rules {
            out.push_str(&format!("## rule id: {}\n\n{}\n\n", r.id, r.body));
        }
    }

    out.push_str(
        "\nWhen your analysis is complete, call the `submit_rule_violations` MCP tool exactly \
         once with `{ violations: [{ rule_id, summary }] }` (an empty array means \"no \
         violations found\"). Do NOT print the result to stdout — the daemon reads it ONLY \
         from `submit_rule_violations`.\n",
    );
    out
}

/// Test-only session runner standing in for the CLI + control socket.
#[cfg(test)]
struct CannedRuleViolationRunner {
    submission: Option<serde_json::Value>,
}

#[cfg(test)]
#[async_trait::async_trait]
impl CorpusCheckSessionRunner for CannedRuleViolationRunner {
    async fn run_session(
        &self,
        _prompt: &str,
    ) -> Result<crate::preflight::corpus_check::CorpusCheckSessionOutcome> {
        Ok(crate::preflight::corpus_check::CorpusCheckSessionOutcome {
            submission: self.submission.clone(),
            stdout_excerpt: String::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LlmProvider;
    use tempfile::TempDir;

    fn write(p: &Path, body: &str) {
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, body).unwrap();
    }

    fn test_model() -> ResolvedModel {
        ResolvedModel {
            provider: LlmProvider::Anthropic,
            model: "claude-test".into(),
            api_base_url: "https://example.invalid".into(),
            api_key: "sk-test".into(),
        }
    }

    fn test_ctx(corpus_dir: PathBuf) -> GlobalRulesCheckCtx {
        GlobalRulesCheckCtx {
            command: "claude".into(),
            model: test_model(),
            prompt_template: "TEST_PROMPT".into(),
            attribution: None,
            retries: 0,
            corpus_dir,
            test_submission: None,
        }
    }

    // ---- payload_to_rule_violations ----

    #[test]
    fn empty_violations_array_maps_to_empty_vec() {
        let payload = serde_json::json!({ "violations": [] });
        let out = payload_to_rule_violations(&payload).expect("empty array deserializes");
        assert!(out.is_empty());
    }

    #[test]
    fn single_violation_is_mapped() {
        let payload = serde_json::json!({
            "violations": [
                { "rule_id": "no-tautological-tests", "summary": "test asserts a constant" }
            ]
        });
        let out = payload_to_rule_violations(&payload).expect("deserializes");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule_id, "no-tautological-tests");
        assert_eq!(out[0].summary, "test asserts a constant");
    }

    #[test]
    fn missing_violations_key_is_correctable_error() {
        let payload = serde_json::json!({ "results": [] });
        let err = payload_to_rule_violations(&payload).expect_err("missing key must error");
        assert!(err.contains("violations"), "got: {err}");
    }

    #[test]
    fn non_array_violations_is_correctable_error() {
        let payload = serde_json::json!({ "violations": "nope" });
        let err = payload_to_rule_violations(&payload).expect_err("non-array must error");
        assert!(err.contains("violations"), "got: {err}");
    }

    #[test]
    fn entry_missing_rule_id_is_correctable_error() {
        let payload = serde_json::json!({ "violations": [ { "summary": "x" } ] });
        let err =
            payload_to_rule_violations(&payload).expect_err("missing required field must error");
        assert!(err.contains("submit_rule_violations"), "got: {err}");
    }

    // ---- corpus loading: flat + grouped ----

    #[test]
    fn load_rule_corpus_flat_and_grouped() {
        let tmp = TempDir::new().unwrap();
        let corpus = tmp.path();
        write(&corpus.join("no-secrets.md"), "Secrets are never committed to the repo.");
        write(
            &corpus.join("testing/no-tautological-tests.md"),
            "Tests must assert real behavior, never tautologies.",
        );
        write(&corpus.join("README.md"), "This is the corpus readme, not a rule.");
        write(&corpus.join("notes.json"), "{ \"ignored\": true }");
        let rules = load_rule_corpus(corpus);
        let ids: Vec<&str> = rules.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["no-secrets", "testing/no-tautological-tests"]);
        assert!(rules[0].body.contains("never committed"));
    }

    #[test]
    fn load_rule_corpus_missing_dir_is_empty() {
        let tmp = TempDir::new().unwrap();
        assert!(load_rule_corpus(&tmp.path().join("nope")).is_empty());
    }

    // ---- corpus resolution ----

    #[test]
    fn resolve_corpus_local_path_ok() {
        let tmp = TempDir::new().unwrap();
        let corpus = tmp.path().join("rules");
        std::fs::create_dir_all(&corpus).unwrap();
        let cache = tmp.path().join("cache");
        let resolved = resolve_corpus(corpus.to_str().unwrap(), &cache).unwrap();
        assert_eq!(resolved, corpus);
    }

    #[test]
    fn resolve_corpus_missing_local_path_errors() {
        let tmp = TempDir::new().unwrap();
        let cache = tmp.path().join("cache");
        let err = resolve_corpus(
            tmp.path().join("nope").to_str().unwrap(),
            &cache,
        )
        .expect_err("missing path must fail fast");
        assert!(format!("{err:#}").contains("does not exist"));
    }

    #[test]
    fn is_git_url_detection() {
        assert!(is_git_url("https://github.com/acme/rules.git"));
        assert!(is_git_url("git@github.com:acme/rules.git"));
        assert!(is_git_url("ssh://git@host/acme/rules"));
        assert!(!is_git_url("/var/lib/autocoder/rules"));
        assert!(!is_git_url("./rules"));
    }

    // ---- prompt construction ----

    #[tokio::test]
    async fn prompt_lists_deltas_and_inlines_rules_and_submit_instruction() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().join("ws");
        write(
            &ws.join("openspec/changes/c1/specs/alpha/spec.md"),
            "## ADDED Requirements\n\n### Requirement: A1\nBody.\n",
        );
        let corpus = tmp.path().join("rules");
        write(&corpus.join("no-secrets.md"), "Secrets are never committed.");
        let prompt = build_global_rules_prompt("PROMPT_TEMPLATE", &ws, "c1", &corpus);
        assert!(prompt.starts_with("PROMPT_TEMPLATE"));
        // The change's deltas are listed as paths (read on demand).
        assert!(prompt.contains("openspec/changes/c1/specs/alpha/spec.md"));
        // The rule corpus is INLINED (id + body), not merely referenced by path.
        assert!(prompt.contains("rule id: no-secrets"));
        assert!(prompt.contains("Secrets are never committed."));
        assert!(prompt.contains("submit_rule_violations"));
        // The delta contents are NOT inlined (read on demand).
        assert!(!prompt.contains("Requirement: A1"));
    }

    #[tokio::test]
    async fn prompt_handles_empty_corpus_gracefully() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().join("ws");
        let corpus = tmp.path().join("rules");
        std::fs::create_dir_all(&corpus).unwrap();
        let prompt = build_global_rules_prompt("PROMPT_TEMPLATE", &ws, "c1", &corpus);
        assert!(prompt.contains("global rule corpus is empty"), "got: {prompt}");
    }

    // ---- orchestration (run_*_with_runner) ----

    #[tokio::test]
    async fn valid_submission_is_found() {
        let tmp = TempDir::new().unwrap();
        let corpus = tmp.path().join("rules");
        write(&corpus.join("no-secrets.md"), "No secrets.");
        let ctx = test_ctx(corpus);
        let runner = CannedRuleViolationRunner {
            submission: Some(serde_json::json!({
                "violations": [ { "rule_id": "no-secrets", "summary": "key in config" } ]
            })),
        };
        let out =
            run_agentic_global_rules_check_with_runner(&ctx, tmp.path(), "c1", &runner).await;
        match out {
            GlobalRulesCheckOutcome::Found(f) => {
                assert_eq!(f.len(), 1);
                assert_eq!(f[0].rule_id, "no-secrets");
            }
            other => panic!("expected Found, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn empty_submission_is_clean() {
        let tmp = TempDir::new().unwrap();
        let ctx = test_ctx(tmp.path().to_path_buf());
        let runner = CannedRuleViolationRunner {
            submission: Some(serde_json::json!({ "violations": [] })),
        };
        let out =
            run_agentic_global_rules_check_with_runner(&ctx, tmp.path(), "c1", &runner).await;
        assert!(matches!(out, GlobalRulesCheckOutcome::Clean), "got {out:?}");
    }

    #[tokio::test]
    async fn no_submission_fails_closed() {
        let tmp = TempDir::new().unwrap();
        let ctx = test_ctx(tmp.path().to_path_buf());
        let runner = CannedRuleViolationRunner { submission: None };
        let out =
            run_agentic_global_rules_check_with_runner(&ctx, tmp.path(), "c1", &runner).await;
        assert!(matches!(out, GlobalRulesCheckOutcome::Errored { .. }), "got {out:?}");
    }

    #[tokio::test]
    #[tracing_test::traced_test]
    async fn fail_closed_diagnostics_carry_the_rules_gate_label() {
        let tmp = TempDir::new().unwrap();
        let ctx = test_ctx(tmp.path().to_path_buf());
        let runner = CannedRuleViolationRunner { submission: None };
        let _ = run_agentic_global_rules_check_with_runner(&ctx, tmp.path(), "c1", &runner).await;
        assert!(
            logs_contain("[verifier:rules]"),
            "the fail-closed WARN must carry the [verifier:rules] gate identifier"
        );
    }

    #[tokio::test]
    async fn unregistered_strategy_fails_closed() {
        let tmp = TempDir::new().unwrap();
        let mut ctx = test_ctx(tmp.path().to_path_buf());
        ctx.model.provider = LlmProvider::Ollama;
        ctx.command = "definitely-not-a-registered-cli".into();
        let out = run_agentic_global_rules_check(&ctx, tmp.path(), "c1").await;
        assert!(matches!(out, GlobalRulesCheckOutcome::Errored { .. }), "got {out:?}");
    }

    // ---- allowed-tools surface ----

    #[test]
    fn allowed_tools_are_read_only_plus_submit_rule_violations() {
        let tools = agentic_global_rules_allowed_tools();
        assert!(tools.contains(&"Read".to_string()));
        assert!(tools.contains(&"Glob".to_string()));
        assert!(tools.contains(&"Grep".to_string()));
        assert!(
            !tools.iter().any(|t| t == "Bash" || t == "Write" || t == "Edit"),
            "sandbox must deny Bash/Write/Edit: {tools:?}"
        );
        assert!(
            tools.iter().any(|t| t.contains("submit_rule_violations")),
            "submit_rule_violations must be allowed: {tools:?}"
        );
    }

    // ---- prompt loader ----

    #[test]
    fn embedded_prompt_is_non_empty_and_names_the_tool() {
        assert!(!EMBEDDED_PROMPT.trim().is_empty());
        assert!(EMBEDDED_PROMPT.contains("submit_rule_violations"));
        assert!(EMBEDDED_PROMPT.contains("global rules") || EMBEDDED_PROMPT.contains("GLOBAL RULES"));
    }

    #[test]
    fn load_prompt_template_none_returns_embedded() {
        assert_eq!(load_prompt_template(None).unwrap(), EMBEDDED_PROMPT);
    }

    #[test]
    fn load_prompt_template_empty_override_is_rejected() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("empty.md");
        std::fs::write(&p, "  \n ").unwrap();
        let err = load_prompt_template(Some(&p)).expect_err("empty override must be rejected");
        assert!(format!("{err:#}").contains("empty"));
    }
}
