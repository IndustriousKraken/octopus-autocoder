//! `autocoder verify <change-slug>` — run the pre-executor verifier-gate
//! checks (`[in]` / `[canon]` / `[rules]`) locally on a working-tree change,
//! BEFORE pushing, so an operator learns whether the change would pass the
//! server gates without a remote round-trip.
//!
//! `verify` is a new INVOCATION SURFACE for the existing gate checks, NOT a
//! reimplementation: it calls the same entry points the daemon's polling
//! loop calls (`preflight::*::run_agentic_*_check`), with the same prompts,
//! the same per-gate model config, AND the same submission schemas — so its
//! verdict matches what the server will enforce.
//!
//! The submission transport (the control socket the gate's MCP child relays
//! its `submit_*` verdict over) is a HARD precondition: without it every
//! gate drains `None` and fails closed. `verify` stands it up in-process via
//! `control_socket::spawn_submission_listener` for the duration of the run.

use crate::config::{Config, ContradictionCheckMode};
use crate::preflight::canon_contradiction::{
    CanonContradictionCheckCtx, CanonContradictionCheckOutcome,
};
use crate::preflight::change_contradiction::{
    ContradictionCheckCtx, ContradictionCheckOutcome,
};
use crate::preflight::global_rules::{GlobalRulesCheckCtx, GlobalRulesCheckOutcome};
use crate::verifier_gate::VerifierGate;
use anyhow::{Context, Result, anyhow, bail};
use std::path::{Path, PathBuf};

/// CLI arguments for `verify`.
pub struct VerifyArgs {
    /// The change slug under `openspec/changes/<slug>/` in the cwd repo.
    pub change_slug: String,
    /// `--all`: run every realized spec-checking gate (`in`, `canon`,
    /// `rules`) regardless of its enabled state in config.
    pub all: bool,
    /// `--gate in,canon`: run exactly the named subset. An unknown name is
    /// an error, not a silent skip. Mutually informative with `--all`.
    pub gate: Option<String>,
    /// Optional config path; when omitted, the daemon's discovery order is
    /// used (so the same minimal config the check-only install drops is
    /// found automatically).
    pub config: Option<PathBuf>,
}

/// One gate's result in a verify run. The verdict is fail-closed: a gate
/// that could not run is `CouldNotRun`, NEVER `Clean`.
#[derive(Debug)]
enum GateResult {
    /// Ran and found no contradictions.
    Clean,
    /// Ran and found contradictions; the human-readable, gate-labeled
    /// finding lines (each carries the narrative the server marker's
    /// `revision_suggestion` would).
    Found(Vec<String>),
    /// Could NOT run (model unconfigured, transport error, unregistered
    /// strategy, no submission captured). Fail-closed.
    CouldNotRun(String),
}

/// The full report of a verify run: one entry per gate that was selected.
#[derive(Debug)]
struct VerifyReport {
    results: Vec<(VerifierGate, GateResult)>,
    /// True when the resolved selected-gate set was empty.
    empty_selection: bool,
}

impl VerifyReport {
    /// CI-usable exit code per the gatekeepers-fail-closed standard:
    /// `0` ONLY when at least one gate ran AND every gate that ran is
    /// clean; non-zero on any finding, any gate that could not run, OR an
    /// empty selection (no gate evaluated the change).
    fn exit_code(&self) -> i32 {
        if self.empty_selection {
            return 2;
        }
        let mut any_clean = false;
        for (_g, r) in &self.results {
            match r {
                GateResult::Clean => any_clean = true,
                GateResult::Found(_) => return 1,
                GateResult::CouldNotRun(_) => return 1,
            }
        }
        if any_clean { 0 } else { 2 }
    }

    /// Render the report to stdout, grouped + labeled by gate.
    fn render(&self) {
        if self.empty_selection {
            println!(
                "✗ verify: no spec-checking gate evaluated the change — \
                 no gate is enabled in config and no selector forced one. \
                 A clean pass is never manufactured for an unchecked change."
            );
            return;
        }
        for (gate, result) in &self.results {
            let label = gate.label();
            match result {
                GateResult::Clean => {
                    println!("✅ {label} clean: no contradictions found");
                }
                GateResult::Found(lines) => {
                    println!("❌ {label} found {} contradiction(s):", lines.len());
                    for line in lines {
                        println!("   {} {line}", gate.label());
                    }
                }
                GateResult::CouldNotRun(cause) => {
                    println!("🚫 {label} could not run (fail-closed): {cause}");
                }
            }
        }
    }
}

/// Entry point invoked from `cli::dispatch`. Resolves config + paths, stands
/// up the submission listener as a precondition, runs the selected gates,
/// renders the report, AND exits with the CI-usable code.
pub async fn execute(args: VerifyArgs) -> Result<()> {
    let config_path = crate::cli::run::resolve_run_config_path(args.config.clone())
        .context("resolving verify config path")?;
    let cfg = Config::load_from(&config_path)
        .with_context(|| format!("loading config from {}", config_path.display()))?;
    let paths = crate::paths::resolve_daemon_paths(&cfg)
        .context("resolving daemon data paths")?;
    crate::paths::ensure_directories(&paths)
        .context("creating daemon data directories")?;

    let workspace = std::env::current_dir().context("resolving current working directory")?;
    let change_dir = workspace
        .join("openspec")
        .join("changes")
        .join(&args.change_slug);
    if !change_dir.is_dir() {
        bail!(
            "change directory {} does not exist; run `verify` in the repository root with a valid change slug",
            change_dir.display()
        );
    }

    let report = run_verify(&cfg, &paths, &workspace, &args.change_slug, args.all, args.gate.as_deref())
        .await?;
    report.render();
    let code = report.exit_code();
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

/// Resolve the set of gates to run. Default (`all=false`, `gate=None`) =
/// the spec-checking gates ENABLED in config. `all=true` = every realized
/// spec-checking gate. `gate=Some("in,canon")` = the named subset (an
/// unknown name is an error). The three spec-checking gates are `In`,
/// `Canon`, `Rules` (`Out` is post-executor and not a `verify` gate).
fn resolve_selected_gates(
    cfg: &Config,
    all: bool,
    gate: Option<&str>,
) -> Result<Vec<VerifierGate>> {
    const SPEC_GATES: [VerifierGate; 3] =
        [VerifierGate::In, VerifierGate::Canon, VerifierGate::Rules];
    if let Some(list) = gate {
        let mut selected = Vec::new();
        for raw in list.split(',') {
            let name = raw.trim();
            if name.is_empty() {
                continue;
            }
            let g = match name {
                "in" => VerifierGate::In,
                "canon" => VerifierGate::Canon,
                "rules" => VerifierGate::Rules,
                other => {
                    return Err(anyhow!(
                        "unknown gate `{other}` in --gate; valid spec-checking gates: in, canon, rules"
                    ));
                }
            };
            if !selected.contains(&g) {
                selected.push(g);
            }
        }
        return Ok(selected);
    }
    if all {
        return Ok(SPEC_GATES.to_vec());
    }
    // Default: the gates enabled in config.
    let mut enabled = Vec::new();
    if matches!(
        cfg.executor.change_internal_contradiction_check,
        ContradictionCheckMode::Enabled
    ) {
        enabled.push(VerifierGate::In);
    }
    if matches!(
        cfg.executor.change_canonical_contradiction_check,
        ContradictionCheckMode::Enabled
    ) {
        enabled.push(VerifierGate::Canon);
    }
    if matches!(
        cfg.executor.global_rules_check,
        ContradictionCheckMode::Enabled
    ) {
        enabled.push(VerifierGate::Rules);
    }
    Ok(enabled)
}

/// Build, stand up the listener, run the selected gates, AND collect the
/// report. The listener guard is held for the whole run; it is dropped at
/// function return, which cancels `serve` and removes the socket file.
async fn run_verify(
    cfg: &Config,
    paths: &crate::paths::DaemonPaths,
    workspace: &Path,
    change_slug: &str,
    all: bool,
    gate: Option<&str>,
) -> Result<VerifyReport> {
    let selected = resolve_selected_gates(cfg, all, gate)?;
    if selected.is_empty() {
        // Loud empty case — never a silent exit 0.
        return Ok(VerifyReport {
            results: Vec::new(),
            empty_selection: true,
        });
    }

    // HARD precondition: without the in-process submission transport every
    // gate drains `None` and fails closed. Held for the whole run.
    let _listener = crate::control_socket::spawn_submission_listener(paths)
        .context("standing up the in-process submission listener (verify precondition)")?;

    let ctxs = build_ctxs(cfg, paths)?;
    let report = evaluate_gates(workspace, change_slug, &selected, &ctxs).await;
    Ok(report)
}

/// The per-gate contexts, each `Some` only when its model config is present
/// AND resolvable. A `None` means the gate cannot run (model unconfigured)
/// — evaluated as fail-closed `CouldNotRun`, never silently skipped.
struct GateCtxs {
    in_ctx: Option<ContradictionCheckCtx>,
    canon_ctx: Option<CanonContradictionCheckCtx>,
    rules_ctx: Option<GlobalRulesCheckCtx>,
}

/// Build the gate contexts from config. The session timeout is resolved
/// from `ExecutorConfig::agentic_session_timeout()` (the unified
/// `executor.agentic_session_timeout_secs`, default 3600) — NOT a
/// verify-local literal. A gate whose LLM block is absent gets `None`
/// (it is reported `CouldNotRun` at evaluation, fail-closed). A resolve or
/// corpus error is propagated so the run fails loudly rather than silently
/// dropping the gate.
fn build_ctxs(cfg: &Config, paths: &crate::paths::DaemonPaths) -> Result<GateCtxs> {
    let timeout = cfg.executor.agentic_session_timeout();
    let retries = cfg.executor.verifier_gate_retries;

    let in_ctx = match cfg.executor.change_internal_contradiction_check_llm.as_ref() {
        Some(llm) => {
            let model = crate::llm::resolve_contradiction_check_model(llm)
                .context("resolving change-internal contradiction-check model")?;
            let prompt_template =
                crate::preflight::change_contradiction::load_prompt_template(
                    cfg.executor
                        .change_internal_contradiction_check_prompt_path
                        .as_deref(),
                )
                .context("loading change-contradiction-check prompt")?;
            Some(ContradictionCheckCtx {
                command: crate::config::resolve_cli_command(
                    &cfg.executor.command,
                    crate::config::default_cli_for(model.provider),
                ),
                model,
                prompt_template,
                attribution: None,
                retries,
                timeout,
                revision_transcript_fetch_retries: cfg
                    .executor
                    .revision_transcript_fetch_retries,
                revision_converge_attempts: cfg.executor.revision_converge_attempts,
                #[cfg(test)]
                test_submission: None,
            })
        }
        None => None,
    };

    let canon_ctx = match cfg.executor.change_canonical_contradiction_check_llm.as_ref() {
        Some(llm) => {
            let model = crate::llm::resolve_canon_contradiction_check_model(llm)
                .context("resolving change-vs-canonical contradiction-check model")?;
            let prompt_template = crate::preflight::canon_contradiction::load_prompt_template(
                cfg.executor
                    .change_canonical_contradiction_check_prompt_path
                    .as_deref(),
            )
            .context("loading change-vs-canonical-check prompt")?;
            Some(CanonContradictionCheckCtx {
                command: crate::config::resolve_cli_command(
                    &cfg.executor.command,
                    crate::config::default_cli_for(model.provider),
                ),
                model,
                prompt_template,
                attribution: None,
                retries,
                timeout,
                #[cfg(test)]
                test_submission: None,
            })
        }
        None => None,
    };

    let rules_ctx = match cfg.executor.global_rules_check_llm.as_ref() {
        Some(llm) => {
            let model = crate::llm::resolve_global_rules_check_model(llm)
                .context("resolving global-rules-check model")?;
            let prompt_template = crate::preflight::global_rules::load_prompt_template(
                cfg.executor.global_rules_check_prompt_path.as_deref(),
            )
            .context("loading global-rules-check prompt")?;
            // The corpus is required by the `[rules]` gate; resolve it from
            // config (cloning a configured git repo into the daemon cache).
            let corpus = cfg.executor.global_rules.corpus.as_deref().ok_or_else(|| {
                anyhow!(
                    "global_rules_check_llm is set but executor.global_rules.corpus is absent; the [rules] gate needs a corpus"
                )
            })?;
            let corpus_cache = paths.cache.join("global-rules-corpus");
            let corpus_dir =
                crate::preflight::global_rules::resolve_corpus(corpus, &corpus_cache)
                    .context("resolving global rule corpus (executor.global_rules.corpus)")?;
            Some(GlobalRulesCheckCtx {
                command: crate::config::resolve_cli_command(
                    &cfg.executor.command,
                    crate::config::default_cli_for(model.provider),
                ),
                model,
                prompt_template,
                attribution: None,
                retries,
                timeout,
                corpus_dir,
                #[cfg(test)]
                test_submission: None,
            })
        }
        None => None,
    };

    Ok(GateCtxs {
        in_ctx,
        canon_ctx,
        rules_ctx,
    })
}

/// Run each selected gate against the working-tree change, reusing the
/// SAME `run_agentic_*` entry points the server's polling loop calls. A
/// gate whose ctx is `None` (model unconfigured) is reported `CouldNotRun`
/// (fail-closed), never silently skipped.
async fn evaluate_gates(
    workspace: &Path,
    change_slug: &str,
    selected: &[VerifierGate],
    ctxs: &GateCtxs,
) -> VerifyReport {
    let mut results = Vec::new();
    for &gate in selected {
        let result = match gate {
            VerifierGate::In => match &ctxs.in_ctx {
                Some(ctx) => map_in(
                    crate::preflight::change_contradiction::run_agentic_contradiction_check(
                        ctx, workspace, change_slug,
                    )
                    .await,
                ),
                None => GateResult::CouldNotRun(
                    "executor.change_internal_contradiction_check_llm is not configured".to_string(),
                ),
            },
            VerifierGate::Canon => match &ctxs.canon_ctx {
                Some(ctx) => map_canon(
                    crate::preflight::canon_contradiction::run_agentic_canon_contradiction_check(
                        ctx, workspace, change_slug,
                    )
                    .await,
                ),
                None => GateResult::CouldNotRun(
                    "executor.change_canonical_contradiction_check_llm is not configured".to_string(),
                ),
            },
            VerifierGate::Rules => match &ctxs.rules_ctx {
                Some(ctx) => map_rules(
                    crate::preflight::global_rules::run_agentic_global_rules_check(
                        ctx, workspace, change_slug,
                    )
                    .await,
                ),
                None => GateResult::CouldNotRun(
                    "executor.global_rules_check_llm is not configured".to_string(),
                ),
            },
            // `Out` is post-executor and never a `verify` spec-checking gate;
            // it is unreachable here because `resolve_selected_gates` only
            // ever yields In/Canon/Rules.
            VerifierGate::Out => GateResult::CouldNotRun(
                "the [out] gate is post-executor and not a verify gate".to_string(),
            ),
        };
        results.push((gate, result));
    }
    VerifyReport {
        results,
        empty_selection: false,
    }
}

fn map_in(outcome: ContradictionCheckOutcome) -> GateResult {
    match outcome {
        ContradictionCheckOutcome::Clean => GateResult::Clean,
        ContradictionCheckOutcome::Found(findings) => GateResult::Found(
            findings
                .into_iter()
                .map(|f| {
                    format!(
                        "{} ⇄ {}: {}",
                        f.requirement_a, f.requirement_b, f.summary
                    )
                })
                .collect(),
        ),
        ContradictionCheckOutcome::Errored { cause } => GateResult::CouldNotRun(cause),
    }
}

fn map_canon(outcome: CanonContradictionCheckOutcome) -> GateResult {
    match outcome {
        CanonContradictionCheckOutcome::Clean => GateResult::Clean,
        CanonContradictionCheckOutcome::Found(findings) => GateResult::Found(
            findings
                .into_iter()
                .map(|f| {
                    format!(
                        "{} vs canonical {}/{}: {}",
                        f.change_requirement,
                        f.canonical_capability,
                        f.canonical_requirement,
                        f.summary
                    )
                })
                .collect(),
        ),
        CanonContradictionCheckOutcome::Errored { cause } => GateResult::CouldNotRun(cause),
    }
}

fn map_rules(outcome: GlobalRulesCheckOutcome) -> GateResult {
    match outcome {
        GlobalRulesCheckOutcome::Clean => GateResult::Clean,
        GlobalRulesCheckOutcome::Found(findings) => GateResult::Found(
            findings
                .into_iter()
                .map(|f| format!("rule {}: {}", f.rule_id, f.summary))
                .collect(),
        ),
        GlobalRulesCheckOutcome::Errored { cause } => GateResult::CouldNotRun(cause),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agentic_run::ResolvedModel;
    use serde_json::json;

    fn dummy_model() -> ResolvedModel {
        ResolvedModel {
            provider: crate::config::LlmProvider::Anthropic,
            model: "claude-test".to_string(),
            api_base_url: String::new(),
            api_key: String::new(),
        }
    }

    /// Build an `[in]` ctx with an injected submission (test seam), so the
    /// gate runs without a CLI subprocess or control socket.
    fn in_ctx_with(submission: Option<Option<serde_json::Value>>) -> ContradictionCheckCtx {
        ContradictionCheckCtx {
            command: "claude".to_string(),
            model: dummy_model(),
            prompt_template: "check".to_string(),
            attribution: None,
            retries: 0,
            timeout: std::time::Duration::from_secs(3600),
            revision_transcript_fetch_retries: 0,
            revision_converge_attempts: 0,
            test_submission: submission,
        }
    }

    fn empty_cfg() -> Config {
        // A config that enables NO spec-checking gate.
        let mut cfg: Config = serde_json::from_value(json!({
            "repositories": [],
            "github": {},
            "executor": {
                "kind": "claude_cli",
                "command": "claude"
            }
        }))
        .expect("minimal config deserializes");
        cfg.executor = crate::config::placeholder_executor_config();
        cfg
    }

    #[tokio::test]
    async fn empty_gate_set_is_loud_nonzero() {
        let cfg = empty_cfg();
        // No gate enabled, no selector → empty selection.
        let selected = resolve_selected_gates(&cfg, false, None).unwrap();
        assert!(selected.is_empty(), "no gate should be selected");
        let report = VerifyReport {
            results: Vec::new(),
            empty_selection: true,
        };
        assert_ne!(report.exit_code(), 0, "empty selection must be non-zero");
        assert_eq!(report.exit_code(), 2);
    }

    #[tokio::test]
    async fn unknown_gate_name_is_error() {
        let cfg = empty_cfg();
        let err = resolve_selected_gates(&cfg, false, Some("in,bogus")).unwrap_err();
        assert!(
            format!("{err:#}").contains("unknown gate `bogus`"),
            "unknown gate must error: {err:#}"
        );
    }

    #[tokio::test]
    async fn selector_overrides_enabled_state() {
        let cfg = empty_cfg(); // nothing enabled
        // --all selects all three even though none are enabled.
        let all = resolve_selected_gates(&cfg, true, None).unwrap();
        assert_eq!(all.len(), 3);
        // --gate names a subset.
        let subset = resolve_selected_gates(&cfg, false, Some("in,canon")).unwrap();
        assert_eq!(subset, vec![VerifierGate::In, VerifierGate::Canon]);
    }

    #[tokio::test]
    async fn default_runs_only_enabled_gates() {
        let mut cfg = empty_cfg();
        cfg.executor.change_internal_contradiction_check =
            ContradictionCheckMode::Enabled;
        let selected = resolve_selected_gates(&cfg, false, None).unwrap();
        assert_eq!(selected, vec![VerifierGate::In]);
    }

    #[tokio::test]
    async fn clean_change_exits_zero() {
        let ctxs = GateCtxs {
            in_ctx: Some(in_ctx_with(Some(Some(json!({ "contradictions": [] }))))),
            canon_ctx: None,
            rules_ctx: None,
        };
        let report = evaluate_gates(
            Path::new("/tmp"),
            "some-change",
            &[VerifierGate::In],
            &ctxs,
        )
        .await;
        assert_eq!(report.exit_code(), 0, "clean change must exit 0");
        assert!(matches!(report.results[0].1, GateResult::Clean));
    }

    #[tokio::test]
    async fn contradicting_change_exits_nonzero_and_labels_finding() {
        let ctxs = GateCtxs {
            in_ctx: Some(in_ctx_with(Some(Some(json!({
                "contradictions": [{
                    "requirement_a": "Requirement Foo",
                    "requirement_b": "Requirement Bar",
                    "summary": "Foo and Bar disagree on the default"
                }]
            }))))),
            canon_ctx: None,
            rules_ctx: None,
        };
        let report = evaluate_gates(
            Path::new("/tmp"),
            "some-change",
            &[VerifierGate::In],
            &ctxs,
        )
        .await;
        assert_eq!(report.exit_code(), 1, "contradiction must be non-zero");
        match &report.results[0].1 {
            GateResult::Found(lines) => {
                assert_eq!(lines.len(), 1);
                assert!(
                    lines[0].contains("Foo and Bar disagree"),
                    "finding narrative must surface: {:?}",
                    lines
                );
            }
            other => panic!("expected Found, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn no_submission_fails_closed() {
        // `Some(None)` simulates "agent never submitted" — exactly the
        // no-listener / no-capture case. The gate must be CouldNotRun, NOT
        // Clean, and the exit must be non-zero.
        let ctxs = GateCtxs {
            in_ctx: Some(in_ctx_with(Some(None))),
            canon_ctx: None,
            rules_ctx: None,
        };
        let report = evaluate_gates(
            Path::new("/tmp"),
            "some-change",
            &[VerifierGate::In],
            &ctxs,
        )
        .await;
        assert_ne!(report.exit_code(), 0, "no submission must fail closed");
        assert!(
            matches!(report.results[0].1, GateResult::CouldNotRun(_)),
            "no submission must be CouldNotRun, got {:?}",
            report.results[0].1
        );
    }

    #[tokio::test]
    async fn unconfigured_gate_fails_closed_not_clean() {
        // A selected gate whose ctx is None (model unconfigured) must be
        // reported CouldNotRun (fail-closed), never Clean.
        let ctxs = GateCtxs {
            in_ctx: None,
            canon_ctx: None,
            rules_ctx: None,
        };
        let report = evaluate_gates(
            Path::new("/tmp"),
            "some-change",
            &[VerifierGate::In],
            &ctxs,
        )
        .await;
        assert_ne!(report.exit_code(), 0);
        assert!(matches!(
            report.results[0].1,
            GateResult::CouldNotRun(_)
        ));
    }

    /// Recursively snapshot every regular file under `root` as
    /// (relative-path, byte-contents), sorted. Used to prove a run left the
    /// workspace byte-for-byte unchanged.
    fn snapshot_tree(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        fn walk(dir: &Path, root: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
            let read = match std::fs::read_dir(dir) {
                Ok(r) => r,
                Err(_) => return,
            };
            for entry in read.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, root, out);
                } else if path.is_file() {
                    let rel = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
                    let bytes = std::fs::read(&path).unwrap_or_default();
                    out.push((rel, bytes));
                }
            }
        }
        let mut out = Vec::new();
        walk(root, root, &mut out);
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// True when `name` appears anywhere under `root` (recursive).
    fn tree_contains_named(root: &Path, name: &str) -> bool {
        let read = match std::fs::read_dir(root) {
            Ok(r) => r,
            Err(_) => return false,
        };
        for entry in read.flatten() {
            let path = entry.path();
            if path.file_name().and_then(|n| n.to_str()) == Some(name) {
                return true;
            }
            if path.is_dir() && tree_contains_named(&path, name) {
                return true;
            }
        }
        false
    }

    /// `verify` is READ-ONLY: even on the `Found` verdict — the very case the
    /// server's executor path turns into a `.needs-spec-revision.json` marker
    /// + a revision run — the verify driver must NOT write that marker, must
    /// NOT invoke the executor, and must NOT create or modify any spec/source
    /// file under the workspace. We drive it through the same `test_submission`
    /// seam the other tests use (a canned `Found` submission, so no CLI
    /// subprocess / executor is ever spawned) over a real temp workspace with
    /// a change + spec-delta, then assert the workspace is byte-for-byte
    /// unchanged and no marker exists anywhere beneath it.
    #[tokio::test]
    async fn verify_is_read_only_even_when_a_gate_finds_contradictions() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ws = tmp.path();
        let change = "ro-change";
        let cap_dir = ws
            .join("openspec/changes")
            .join(change)
            .join("specs/some-capability");
        std::fs::create_dir_all(&cap_dir).unwrap();
        std::fs::write(
            cap_dir.join("spec.md"),
            "## ADDED Requirements\n### Requirement: Foo\nThe system SHALL foo.\n\n#### Scenario: foos\n- **WHEN** asked\n- **THEN** it foos\n",
        )
        .unwrap();
        // A bystander source file: it must survive untouched too.
        std::fs::write(ws.join("src_marker.txt"), b"original source bytes").unwrap();

        let before = snapshot_tree(ws);

        // A `Found` verdict — the case the server WOULD persist a marker for.
        let ctxs = GateCtxs {
            in_ctx: Some(in_ctx_with(Some(Some(json!({
                "contradictions": [{
                    "requirement_a": "Foo",
                    "requirement_b": "Bar",
                    "summary": "Foo and Bar disagree"
                }]
            }))))),
            canon_ctx: None,
            rules_ctx: None,
        };
        let report = evaluate_gates(ws, change, &[VerifierGate::In], &ctxs).await;

        // Sanity: the gate really did find the contradiction (so we are
        // exercising the marker-writing-temptation path, not a no-op).
        assert!(
            matches!(report.results[0].1, GateResult::Found(_)),
            "fixture must produce a Found verdict, got {:?}",
            report.results[0].1
        );

        // Read-only guarantee 1: no `.needs-spec-revision.json` marker written
        // anywhere under the workspace.
        assert!(
            !tree_contains_named(ws, ".needs-spec-revision.json"),
            "verify must NOT write a .needs-spec-revision.json marker"
        );

        // Read-only guarantee 2: the workspace file tree is byte-for-byte
        // identical — no spec/source file created or modified by the run.
        let after = snapshot_tree(ws);
        assert_eq!(
            before, after,
            "verify must not create or modify any file under the workspace"
        );
    }

    #[tokio::test]
    async fn timeout_resolved_from_config_not_literal() {
        // A configured agentic_session_timeout_secs flows into the gate ctx.
        let mut cfg = empty_cfg();
        cfg.executor.agentic_session_timeout_secs = 42;
        cfg.executor.change_internal_contradiction_check_llm = Some(
            serde_json::from_value(json!({
                "provider": "anthropic",
                "model": "claude-test"
            }))
            .unwrap(),
        );
        let (_td, paths) = crate::testing::test_daemon_paths();
        let ctxs = build_ctxs(&cfg, &paths).unwrap();
        let in_ctx = ctxs.in_ctx.expect("in ctx built");
        assert_eq!(
            in_ctx.timeout,
            std::time::Duration::from_secs(42),
            "timeout must come from config, not a literal"
        );

        // Absent → the unified default of 3600.
        let mut cfg2 = empty_cfg();
        cfg2.executor.agentic_session_timeout_secs =
            crate::config::default_agentic_session_timeout();
        cfg2.executor.change_internal_contradiction_check_llm = Some(
            serde_json::from_value(json!({
                "provider": "anthropic",
                "model": "claude-test"
            }))
            .unwrap(),
        );
        let ctxs2 = build_ctxs(&cfg2, &paths).unwrap();
        assert_eq!(
            ctxs2.in_ctx.unwrap().timeout,
            std::time::Duration::from_secs(3600)
        );
    }
}
