//! AI-driven code-quality reviewer. Sends a structured `ReviewContext`
//! (changed-file contents + change-spec context + diff) to a configured LLM
//! and parses the response into a `ReviewReport`. Scope is deliberately
//! code-quality only; spec compliance is a separate verification concern.

use crate::config::{LlmProvider, ReviewerConfig, ReviewerKind};
use crate::llm::{self, LlmClient};
use crate::prompts::{PromptId, PromptLoader};
use anyhow::{Context, Result, anyhow};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

mod agentic;
// Re-exported so call sites outside the reviewer keep their `crate::code_reviewer::*`
// paths. Items used only by the moved unit tests (`agentic_review_allowed_tools`,
// `AGENTIC_REVIEW_ALLOWED_TOOLS`, `run_agentic_review_with_runner`) are reached
// directly through the `agentic` module from the sibling test file instead.
pub use agentic::{
    AgenticReviewOutcome, REVIEWER_ROLE, render_agentic_review_prompt, run_agentic_review,
};
pub(crate) use agentic::{
    CliReviewSessionRunner, ReviewSessionRunner, payload_to_review_result,
    resolve_reviewer_strategy,
};

/// Built-in default prompt template, embedded at compile time so the
/// binary runs without requiring `prompts/` on the filesystem. The
/// [`PromptLoader`] also references the same file via `include_str!`;
/// this alias remains here so existing anti-drift tests can compare
/// the reviewer's resolved template against the embedded source of
/// truth.
#[cfg(test)]
const DEFAULT_TEMPLATE: &str = include_str!("../../prompts/code-review-default.md");

/// Backwards-compatible default for the reviewer's prompt-body cap. The
/// real cap is read from `ReviewerConfig::prompt_budget_chars`; this
/// constant exists as the resolution target for `serde(default = ...)`
/// and the documented baseline. Operators override via `config.yaml`.
const DEFAULT_PROMPT_BUDGET: usize = 2_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewVerdict {
    Pass,
    Concerns,
    Block,
    /// The reviewer could not produce a verdict — the agentic session was
    /// discarded (no valid `submit_review`) or errored. Per the
    /// gatekeepers-fail-closed standard this is a distinct, VISIBLE,
    /// non-passing state (rendered as a `## Code Review: FAILED TO RUN`
    /// section AND a `FAILED TO RUN` ledger line) — NOT an approval, NOT a
    /// silent omission.
    FailedToRun,
}

#[derive(Debug, Clone)]
pub struct ReviewReport {
    pub verdict: ReviewVerdict,
    pub markdown: String,
    /// Structured per-concern records the reviewer-initiated revision
    /// pipeline reads. Populated from a trailing fenced YAML block in the
    /// LLM response (info string `revision-requests`). Older templates that
    /// don't emit the block produce an empty vec, which keeps the
    /// reviewer-initiated revision flow off for that operator's setup.
    pub concerns: Vec<ReviewConcern>,
    /// When populated, the report represents a per-change reviewer pass:
    /// each element is one `(change-slug, per-change markdown)` pair and
    /// the PR-body composer emits one `## Code Review: <slug>` section
    /// per element INSTEAD OF a single combined `## Code Review` block.
    /// Empty for bundled-mode reports.
    pub per_change_sections: Vec<PerChangeSection>,
    /// Redaction-safe `<provider>/<model>` model attribution for the
    /// reviewer that produced this report (a49). `Some` when the reviewer
    /// was built from a config carrying a `(provider, model)`; the PR-body
    /// composer renders it as `*Reviewer: <provider>/<model>*`. `None` for
    /// reports built without a configured reviewer (e.g. test fixtures or
    /// the reviewer-failed synthetic report), in which case no attribution
    /// line is emitted.
    pub attribution: Option<String>,
}

/// One per-change reviewer section, surfaced into the PR body under a
/// `## Code Review: <change_slug>` heading. The `markdown` body includes
/// the per-change verdict + concerns + revision-requests in the same
/// format the bundled-mode `## Code Review` block uses.
#[derive(Debug, Clone)]
pub struct PerChangeSection {
    pub change_slug: String,
    pub markdown: String,
}

/// One concern parsed from the reviewer's structured `revision-requests`
/// block. The `summary` mirrors the bullet text in the existing markdown
/// section; `actionable_request` + `should_request_revision` are the
/// per-concern signals the reviewer-initiated revision pipeline reads.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewConcern {
    pub summary: String,
    #[serde(default)]
    pub actionable_request: Option<String>,
    #[serde(default)]
    pub should_request_revision: bool,
    /// The reviewer's own structured security signal (a004). `true` when the
    /// reviewer classified this finding as a credential/secret/key exposure
    /// or an injection vulnerability. The verdict-handling path escalates the
    /// effective verdict to `Block` when any concern carries this flag (see
    /// [`concerns_flag_security_critical`]) — keyed on this signal, NEVER on a
    /// substring scan of `summary`. `#[serde(default)]` keeps older
    /// reviewer templates (which omit the field) parsing as `false`.
    #[serde(default)]
    pub security_critical: bool,
    /// Per-change attribution: in per_change reviewer mode, set to the
    /// change slug whose review surfaced this concern. Used by the
    /// dropped-cap annotator to write the "(not auto-revised; cap
    /// budget exhausted)" footer into the correct
    /// `## Code Review: <slug>` PR-body section. `None` in bundled
    /// mode (annotations land in the single `## Code Review` block).
    #[serde(default)]
    pub change_slug: Option<String>,
}

/// One archived OpenSpec change's source material. Used to give the
/// reviewer the *intent* of the change, not just the mechanical diff.
#[derive(Debug, Clone)]
pub struct ChangeBrief {
    pub name: String,
    pub proposal: String,
    pub design: Option<String>,
    pub tasks: String,
}

/// One file modified by the pass, captured at the agent-branch state.
#[derive(Debug, Clone)]
pub struct ChangedFile {
    pub path: String,
    pub contents: String,
}

/// The reviewer's input is a review SURFACE that is EITHER a unified diff
/// (the per-pass review, OR an on-demand review of a PR or commit) OR an
/// on-demand TARGET (a file, a file-set, OR a described area) carrying NO
/// diff. A `ReviewContext` whose `target` is `None` is a diff-based review
/// (the historical shape); a `Some(_)` target is a no-diff target review
/// whose surface is the operator's review focus plus the target file-path
/// list rendered IN PLACE OF a diff. In both cases the agent reads files on
/// demand via `Read`/`Glob`/`Grep`.
#[derive(Debug, Clone)]
pub enum ReviewTarget {
    /// A concrete file-set: the operator named the files. The reviewer reads
    /// the CURRENT content of each path on demand (no diff). `paths` are
    /// workspace-relative.
    Files { paths: Vec<String> },
    /// A free-text description of functionality/area. The reviewer LOCATES
    /// the relevant files itself via `Glob`/`Grep` during the session AND
    /// names the files it actually reviewed in its verdict.
    Description { focus: String },
}

impl ReviewTarget {
    /// The operator's stated review focus rendered in place of a diff. For a
    /// file-set this is a fixed instruction (the file list carries the
    /// scope); for a description it is the description itself.
    fn focus_text(&self) -> String {
        match self {
            ReviewTarget::Files { paths } => format!(
                "Review the CURRENT content of the {} target file(s) listed below \
                 (this is NOT a diff — review the files as they are now).",
                paths.len()
            ),
            ReviewTarget::Description { focus } => focus.clone(),
        }
    }
}

/// All the material the reviewer sees: the change(s) that shipped, the
/// resulting file state, and the unified diff. Rendering into a prompt
/// honors `ReviewerConfig::prompt_budget_chars` in priority order
/// (context > files > diff).
///
/// `target` is the on-demand TARGET review surface (a59): when `Some`, the
/// review carries NO diff and the rendered prompt presents the operator's
/// focus + the target file-path list in place of a diff. When `None` the
/// context is a diff-based review and rendering is unchanged.
#[derive(Debug, Clone, Default)]
pub struct ReviewContext {
    pub archived_changes: Vec<ChangeBrief>,
    pub changed_files: Vec<ChangedFile>,
    pub diff: String,
    pub target: Option<ReviewTarget>,
}

/// Per-change reviewer call: the change being reviewed (own brief, own
/// diff, own touched files) plus the cross-change preamble naming the
/// other changes in the same pass. Used only when
/// `ReviewerConfig::mode == PerChange`.
#[derive(Debug, Clone)]
pub struct PerChangeContext {
    /// The change being reviewed in this call.
    pub change_slug: String,
    /// The single-change review context (briefs/files/diff scoped to
    /// this change alone).
    pub context: ReviewContext,
    /// Fixed-size cross-change preamble inserted at the top of the
    /// rendered prompt. Format: human-readable lines describing the
    /// OTHER changes in the same PR (slug + first-paragraph-of-Why),
    /// each truncated to 200 chars. Empty when the pass is single-
    /// change (no other changes to reference).
    pub cross_change_preamble: String,
}

impl ReviewConcern {
    /// Whether this concern is an actionable reviewer-initiated revision
    /// request: it carries `should_request_revision: true` AND a non-empty
    /// (whitespace-trimmed) `actionable_request`. The auto-revise aggregation
    /// (a005) collects every revisable concern from one review into a single
    /// revision run. The verdict is NOT consulted here — verdict gating is
    /// the caller's `auto_revise` tri-state decision.
    pub fn is_revisable(&self) -> bool {
        self.should_request_revision
            && self
                .actionable_request
                .as_deref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false)
    }
}

/// One per-change reviewer result. Returned by `run_per_change_review`
/// for each change in the pass; the PR-body composer turns each one
/// into a `## Code Review: <change-slug>` section.
#[derive(Debug, Clone)]
pub struct PerChangeReview {
    pub change_slug: String,
    pub report: ReviewReport,
}

pub struct CodeReviewer {
    client: Box<dyn LlmClient>,
    template: String,
    auto_revise: crate::config::AutoRevise,
    prompt_budget: usize,
    mode: crate::config::ReviewerMode,
    /// Per-PR cap on operator-initiated re-reviews. `None` means UNLIMITED
    /// (the default) — re-reviews are deliberate operator actions with no
    /// runaway path, so the cap is opt-in only.
    max_code_reviews_per_pr: Option<u32>,
    suggest_rereview_threshold: Option<f32>,
    /// a34 §6: cost-optimization knob. When `true`, the polling
    /// iteration skips the reviewer call for any PR whose diff lives
    /// entirely under `openspec/`. Defaults to `false`.
    skip_spec_only_prs: bool,
    /// Redaction-safe `<provider>/<model>` attribution (a49), stamped onto
    /// every [`ReviewReport`] this reviewer produces. `Some` when built via
    /// [`CodeReviewer::from_config`]; `None` for the test-only
    /// [`CodeReviewer::new`] path (no config, no model to attribute).
    attribution: Option<String>,
    /// a58: reviewer transport. `Oneshot` (default) keeps the existing HTTP
    /// path; `Agentic` routes through the shared `agentic_run` primitive.
    kind: ReviewerKind,
    /// a58: the CLI binary the agentic path wraps (default `claude`).
    command: String,
    /// a58: the reviewer's LLM provider, used to resolve the agentic CLI
    /// strategy via the a55 `provider → CLI` rule. Anthropic for the
    /// test-only [`CodeReviewer::new`] path.
    provider: LlmProvider,
    /// a67: file/function line thresholds for the advisory size flag. The
    /// reviewer appends a `## Size advisory` note when a pass pushes a
    /// changed file or function past these, OR grows one already over.
    /// These reference the size budget defined by the `Source files and
    /// functions stay within a size budget` requirement — its single
    /// canonical home — rather than restating numbers; the defaults track
    /// that budget's targets.
    file_lines_threshold: u64,
    function_lines_threshold: u64,
    /// The reviewer's fully-resolved model (provider, model id, base URL,
    /// resolved key), threaded to the agentic session so the wrapped CLI runs
    /// the OPERATOR-configured model — not the CLI's own default. `Some` when
    /// built via [`CodeReviewer::from_config`] (resolved from the reviewer
    /// config exactly like the verifier gates); `None` for the test-only
    /// [`CodeReviewer::new`] path. When `None`, the agentic session passes
    /// `model: None` (legacy behavior: the CLI picks its default).
    resolved_model: Option<crate::agentic_run::ResolvedModel>,
    /// Wall-clock cap for one agentic reviewer session, resolved from the
    /// SINGLE `executor.agentic_session_timeout_secs` (shared with the verifier
    /// gates AND the revision sessions). The reviewer is built from
    /// `ReviewerConfig`, which does not carry the executor block, so both
    /// production construction sites (startup in `cli::run`, reload in
    /// `control_socket::build_reviewer`) set this from `cfg.executor` via
    /// [`CodeReviewer::with_agentic_session_timeout`]. The test-only
    /// [`CodeReviewer::new`] path defaults it to the resolved one-hour default.
    /// The oneshot path has no analogous timeout (the HTTP client owns it);
    /// this bounds the wrapped CLI subprocess the way the gates bound theirs.
    agentic_session_timeout: Duration,
}

impl CodeReviewer {
    pub fn new(client: Box<dyn LlmClient>, template: String) -> Self {
        Self {
            client,
            template,
            // Test-only constructor: default to `Off` so a reviewer built via
            // `new()` does not auto-revise unless a test opts in with
            // `with_auto_revise`. (The config-driven default is `Block`; see
            // `AutoRevise::default`.)
            auto_revise: crate::config::AutoRevise::Off,
            prompt_budget: DEFAULT_PROMPT_BUDGET,
            mode: crate::config::ReviewerMode::Bundled,
            max_code_reviews_per_pr: None,
            suggest_rereview_threshold: None,
            skip_spec_only_prs: false,
            attribution: None,
            kind: ReviewerKind::Oneshot,
            command: "claude".to_string(),
            provider: LlmProvider::Anthropic,
            file_lines_threshold: crate::audits::code_metrics::DEFAULT_FILE_LINES_THRESHOLD,
            function_lines_threshold: crate::audits::code_metrics::DEFAULT_FUNCTION_LINES_THRESHOLD,
            resolved_model: None,
            // Test-only constructor: default to the resolved one-hour default.
            // No second `3600` literal — this routes through the single config
            // default. Production overrides via `with_agentic_session_timeout`.
            agentic_session_timeout: Duration::from_secs(
                crate::config::default_agentic_session_timeout(),
            ),
        }
    }

    /// Builder-style setter for the agentic reviewer session timeout, resolved
    /// from `executor.agentic_session_timeout_secs`. Both production
    /// construction sites call this with `cfg.executor.agentic_session_timeout()`
    /// so the reviewer shares the ONE timeout with the verifier gates AND the
    /// revision sessions.
    pub fn with_agentic_session_timeout(mut self, timeout: Duration) -> Self {
        self.agentic_session_timeout = timeout;
        self
    }

    /// Builder-style setter for the advisory size-flag thresholds (a67).
    /// Defaults track the size budget (`Source files and functions stay
    /// within a size budget`); `from_config` leaves the defaults in place since
    /// `ReviewerConfig` carries no per-reviewer override. Exercised by the
    /// size-advisory tests.
    #[allow(dead_code)]
    pub fn with_size_thresholds(mut self, file_lines: u64, function_lines: u64) -> Self {
        self.file_lines_threshold = file_lines;
        self.function_lines_threshold = function_lines;
        self
    }

    /// Builder-style setter for the reviewer transport (`oneshot` vs
    /// `agentic`). `from_config` sets this from `reviewer.kind`.
    pub fn with_kind(mut self, kind: ReviewerKind) -> Self {
        self.kind = kind;
        self
    }

    /// Read the configured reviewer transport.
    pub fn kind(&self) -> ReviewerKind {
        self.kind
    }

    /// The resolved agentic reviewer session timeout (from
    /// `executor.agentic_session_timeout_secs`). Exposed so the config-to-
    /// call-site wiring is observable in tests.
    pub fn agentic_session_timeout(&self) -> Duration {
        self.agentic_session_timeout
    }

    /// Builder-style setter for the agentic CLI command (`reviewer.command`).
    pub fn with_command(mut self, command: String) -> Self {
        self.command = command;
        self
    }

    /// Builder-style setter for the reviewer's LLM provider, used to resolve
    /// the agentic CLI strategy via the a55 `provider → CLI` rule.
    pub fn with_provider(mut self, provider: LlmProvider) -> Self {
        self.provider = provider;
        self
    }

    /// Builder-style setter for the reviewer's fully-resolved model, threaded
    /// into the agentic session so the wrapped CLI runs the operator-configured
    /// model. `from_config` sets this from the reviewer config (resolved via
    /// [`crate::llm::resolve_reviewer_model`], identical to the verifier gates);
    /// agentic tests use it directly to assert the `--model` selection.
    pub fn with_resolved_model(
        mut self,
        model: Option<crate::agentic_run::ResolvedModel>,
    ) -> Self {
        self.resolved_model = model;
        self
    }

    /// Builder-style setter for the redaction-safe model attribution (a49).
    /// `from_config` sets this from the reviewer config's `(provider,
    /// model)`. The attribution is stamped onto every [`ReviewReport`] this
    /// reviewer produces and carried into [`ReviewResult`], from which the
    /// initial-review and rerun composers render the `*Reviewer: …*` line.
    pub fn with_attribution(mut self, attribution: Option<String>) -> Self {
        self.attribution = attribution;
        self
    }

    /// Builder-style setter for the per-PR re-review cap. `None` means
    /// unlimited (the default).
    pub fn with_max_code_reviews_per_pr(mut self, cap: Option<u32>) -> Self {
        self.max_code_reviews_per_pr = cap;
        self
    }

    /// Builder-style setter for the diff-overlap re-review suggestion threshold.
    pub fn with_suggest_rereview_threshold(mut self, t: Option<f32>) -> Self {
        self.suggest_rereview_threshold = t;
        self
    }

    /// Per-PR cap on operator-initiated re-reviews (a33/a47). `None` means
    /// unlimited (the default).
    pub fn max_code_reviews_per_pr(&self) -> Option<u32> {
        self.max_code_reviews_per_pr
    }

    /// Optional diff-overlap threshold for the post-revision re-review
    /// suggestion (a33). `None` disables the suggestion.
    pub fn with_skip_spec_only_prs(mut self, b: bool) -> Self {
        self.skip_spec_only_prs = b;
        self
    }

    pub fn skip_spec_only_prs(&self) -> bool {
        self.skip_spec_only_prs
    }

    pub fn suggest_rereview_threshold(&self) -> Option<f32> {
        self.suggest_rereview_threshold
    }

    /// Builder-style setter for the prompt-budget cap. Default is
    /// `DEFAULT_PROMPT_BUDGET` (2,000,000 chars); production callers
    /// always thread the value from `ReviewerConfig::prompt_budget_chars`.
    pub fn with_prompt_budget(mut self, budget: usize) -> Self {
        self.prompt_budget = budget;
        self
    }

    /// Read the resolved prompt-budget cap (in chars). Used by the
    /// hot-reload tests to verify a config-driven update reached the
    /// live reviewer slot.
    #[allow(dead_code)]
    pub fn prompt_budget(&self) -> usize {
        self.prompt_budget
    }

    /// Builder-style setter for the reviewer dispatch mode.
    pub fn with_mode(mut self, mode: crate::config::ReviewerMode) -> Self {
        self.mode = mode;
        self
    }

    /// Read the configured dispatch mode (`Bundled` vs `PerChange`).
    pub fn mode(&self) -> crate::config::ReviewerMode {
        self.mode
    }

    /// Builder-style setter mirroring the tri-state config field of the
    /// same name (a005). It controls whether — AND under which verdict —
    /// concerns marked `should_request_revision` (with a non-empty
    /// `actionable_request`) get forwarded (aggregated into a single
    /// revision run) to the revision dispatcher. Used by `from_config` to
    /// propagate `ReviewerConfig::auto_revise` onto the constructed
    /// reviewer; tests use it directly when they need a specific mode
    /// without round-tripping a full config.
    pub fn with_auto_revise(mut self, mode: crate::config::AutoRevise) -> Self {
        self.auto_revise = mode;
        self
    }

    /// The reviewer-initiated revision mode for this reviewer instance
    /// (a005 tri-state). Read by the posting step that turns actionable
    /// concerns into the single aggregated `<!-- reviewer-revision -->` PR
    /// comment; the caller combines it with the review's verdict via
    /// [`crate::config::AutoRevise::fires`].
    pub fn auto_revise(&self) -> crate::config::AutoRevise {
        self.auto_revise
    }

    /// Wire a reviewer from config: build the LLM client, load the
    /// prompt template via the uniform [`PromptLoader`] (a24). The
    /// loader walks `reviewer.code_review.prompt_path` (nested form)
    /// → `reviewer.prompt_template_path` (legacy flat) → embedded
    /// default; missing/empty configured paths emit a one-shot WARN
    /// AND fall back to the next level.
    pub fn from_config(cfg: &ReviewerConfig) -> Result<Self> {
        let client = llm::build_from_config(cfg)?;
        let template = PromptLoader::load(
            PromptId::CodeReview,
            cfg.code_review.as_ref().and_then(|b| b.prompt_path.as_deref()),
            cfg.prompt_template_path.as_deref(),
            None,
        );
        Ok(Self::new(client, template)
            .with_auto_revise(cfg.auto_revise)
            .with_prompt_budget(cfg.prompt_budget_chars)
            .with_mode(cfg.mode)
            .with_max_code_reviews_per_pr(cfg.max_code_reviews_per_pr)
            .with_suggest_rereview_threshold(cfg.suggest_rereview_threshold)
            .with_skip_spec_only_prs(cfg.skip_spec_only_prs)
            .with_kind(cfg.kind)
            .with_command(cfg.command.clone())
            // After `Config::load_from`, `provider` is always resolved; the
            // unwrap default mirrors the field's documented post-load
            // invariant.
            .with_provider(cfg.provider.unwrap_or(LlmProvider::Anthropic))
            // Resolve the reviewer's model exactly like the verifier gates, so
            // the agentic session runs the operator-configured model instead of
            // the wrapped CLI's own default (the bug this fixes). The api_key is
            // resolved `optional` inside the resolver: keyless → EMPTY (opencode
            // keyless path), configured → the resolved key passed to the CLI.
            .with_resolved_model(Some(llm::resolve_reviewer_model(cfg)?))
            .with_attribution(Some(crate::attribution::AttributionSurface::attribution(cfg))))
    }

    pub async fn review(&self, context: &ReviewContext) -> Result<ReviewReport> {
        self.review_with_preamble(context, "").await
    }

    /// Per-change dispatch: invokes the LLM once per `PerChangeContext`,
    /// each with that change's diff + touched files + the cross-change
    /// preamble. Each call respects `prompt_budget_chars` independently,
    /// so one change's huge file does NOT affect the other changes'
    /// reviews. Returns one `PerChangeReview` per input context, in
    /// input order; transient failures surface as `Err` for the whole
    /// pass (the polling-loop synthesizes a Concerns-verdict report).
    pub async fn review_per_change(
        &self,
        contexts: &[PerChangeContext],
    ) -> Result<Vec<PerChangeReview>> {
        let mut out = Vec::with_capacity(contexts.len());
        for pcc in contexts {
            let report = self
                .review_with_preamble(&pcc.context, &pcc.cross_change_preamble)
                .await
                .with_context(|| format!("per-change review for `{}`", pcc.change_slug))?;
            out.push(PerChangeReview {
                change_slug: pcc.change_slug.clone(),
                report,
            });
        }
        Ok(out)
    }

    /// Run the reviewer against `context`, optionally prepending
    /// `preamble` to the rendered prompt (used by per-change mode to
    /// carry the cross-change context block). An empty preamble is the
    /// bundled-mode behavior.
    pub async fn review_with_preamble(
        &self,
        context: &ReviewContext,
        preamble: &str,
    ) -> Result<ReviewReport> {
        let rendered = render_sections(context, self.prompt_budget);
        // Single-pass substitution (a002): a `{{...}}` token appearing
        // inside a substituted value — most importantly inside the
        // `{{changed_files}}` value when the change under review touches a
        // template, docs, OR the reviewer's own code/specs — is emitted
        // verbatim, never re-expanded. Chained `.replace` re-scanned
        // injected content and could multiply the prompt past the model's
        // context limit.
        let body = crate::prompts::render_template(
            &self.template,
            &[
                ("cross_change_preamble", preamble),
                ("change_context", &rendered.change_context),
                ("changed_files", &rendered.changed_files),
                ("diff", &rendered.diff_or_explanation),
            ],
        );
        log_prompt_stats(context, &rendered, body.len(), self.prompt_budget);
        let raw = self.client.complete(&body).await?;
        let mut report = parse_response(&raw);
        // Stamp the reviewer's redaction-safe attribution (a49) so the
        // PR-body composer can render `*Reviewer: <provider>/<model>*`.
        report.attribution = self.attribution.clone();
        // a67: advisory, non-blocking size flag. Appended to the markdown
        // AFTER the verdict/markdown are assembled; the verdict is never
        // touched (size is a maintainability signal, not a correctness
        // defect).
        append_size_advisory(
            &mut report,
            context,
            self.file_lines_threshold,
            self.function_lines_threshold,
        );
        Ok(report)
    }
}

/// Append the advisory `## Size advisory` section to `report.markdown`
/// when this pass pushes a changed file or function past a size threshold
/// (or grows one already over it). The `verdict` is NOT modified — size
/// is a maintainability signal, not a correctness defect. A no-op when no
/// changed file/function is both over-threshold AND net-grown by the pass.
fn append_size_advisory(
    report: &mut ReviewReport,
    ctx: &ReviewContext,
    file_threshold: u64,
    function_threshold: u64,
) {
    if let Some(section) = size_advisory_section(ctx, file_threshold, function_threshold) {
        if report.markdown.trim().is_empty() {
            report.markdown = section;
        } else {
            report.markdown.push_str("\n\n");
            report.markdown.push_str(&section);
        }
    }
}

/// Net additions/deletions for one file in the unified diff, plus its
/// hunks (used to attribute growth to individual functions).
#[derive(Debug, Default, Clone)]
struct FileDiffStats {
    additions: u64,
    deletions: u64,
    hunks: Vec<DiffHunk>,
}

impl FileDiffStats {
    /// Net lines the pass added to the file (`additions − deletions`).
    fn net(&self) -> i64 {
        self.additions as i64 - self.deletions as i64
    }
}

/// One unified-diff hunk's new-file footprint AND its add/delete counts.
#[derive(Debug, Clone)]
struct DiffHunk {
    /// First new-file line number the hunk covers (1-based).
    new_start: usize,
    /// Last new-file line number the hunk covers (1-based, inclusive).
    /// For a pure-deletion hunk this is `new_start − 1` (no new lines).
    new_end: usize,
    additions: u64,
    deletions: u64,
}

/// Compute the advisory `## Size advisory` markdown section for a review,
/// or `None` when nothing crosses a threshold with net growth. For each
/// changed file the reviewer determines — from the file's full contents
/// AND the unified diff — whether the file (or a function within it)
/// exceeds the file/function threshold AND whether this pass added net
/// lines to it; only files/functions that are BOTH over-threshold AND
/// net-grown are reported. Pure (no I/O) for testability.
fn size_advisory_section(
    ctx: &ReviewContext,
    file_threshold: u64,
    function_threshold: u64,
) -> Option<String> {
    let per_file = parse_unified_diff(&ctx.diff);
    let mut items: Vec<String> = Vec::new();
    for file in &ctx.changed_files {
        let ext = file_extension(&file.path);
        let stats = per_file.get(&file.path);
        let total = file.contents.lines().count() as u64;
        // Whole-file advisory.
        let file_net = stats.map(|s| s.net()).unwrap_or(0);
        if total > file_threshold && file_net > 0 {
            match crate::audits::code_metrics::production_test_line_split(&file.contents, &ext) {
                Some((prod, test)) => items.push(format!(
                    "- `{}` is now {total} lines (production {prod} / test {test}); this pass added net lines.",
                    file.path
                )),
                None => items.push(format!(
                    "- `{}` is now {total} lines; this pass added net lines.",
                    file.path
                )),
            }
        }
        // Function-level advisories.
        let hunks: &[DiffHunk] = stats.map(|s| s.hunks.as_slice()).unwrap_or(&[]);
        for span in crate::audits::code_metrics::function_line_spans(&file.contents, &ext) {
            let n = span.line_count();
            if n <= function_threshold {
                continue;
            }
            if function_net_lines(hunks, span.start_line, span.end_line) > 0 {
                items.push(format!(
                    "- function `{}` in `{}` is now {n} lines; this pass added net lines.",
                    span.name, file.path
                ));
            }
        }
    }
    if items.is_empty() {
        return None;
    }
    let mut out = String::from("## Size advisory\n\n");
    out.push_str(&items.join("\n"));
    Some(out)
}

/// Net lines (`additions − deletions`) the pass contributed to a function
/// spanning new-file lines `[fstart, fend]`, attributed by hunk overlap:
/// every hunk whose new-file footprint intersects the span contributes
/// its add/delete counts.
fn function_net_lines(hunks: &[DiffHunk], fstart: usize, fend: usize) -> i64 {
    let mut adds: i64 = 0;
    let mut dels: i64 = 0;
    for h in hunks {
        // A pure-deletion hunk has new_end == new_start - 1; clamp so the
        // overlap test treats it as the single insertion point new_start.
        let hend = h.new_end.max(h.new_start);
        if fstart <= hend && h.new_start <= fend {
            adds += h.additions as i64;
            dels += h.deletions as i64;
        }
    }
    adds - dels
}

/// Parse a unified diff into per-file add/delete totals AND hunk
/// footprints, keyed by the new-file path (the `+++ b/<path>` line with
/// its `a/`/`b/` prefix stripped). Robust to git's extended headers
/// (`diff --git`, `index`, mode/rename lines) AND to hunk headers that
/// omit the optional `,count`.
fn parse_unified_diff(diff: &str) -> std::collections::HashMap<String, FileDiffStats> {
    use std::collections::HashMap;
    static HUNK_RE: OnceLock<Regex> = OnceLock::new();
    let hunk_re = HUNK_RE
        .get_or_init(|| Regex::new(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,\d+)? @@").unwrap());
    let mut map: HashMap<String, FileDiffStats> = HashMap::new();
    let mut current: Option<String> = None;
    let mut cur_new: usize = 0;
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("+++ ") {
            current = normalize_diff_path(rest);
            if let Some(p) = &current {
                map.entry(p.clone()).or_default();
            }
            continue;
        }
        if line.starts_with("--- ")
            || line.starts_with("diff --git")
            || line.starts_with("index ")
            || line.starts_with("new file")
            || line.starts_with("deleted file")
            || line.starts_with("rename ")
            || line.starts_with("similarity ")
            || line.starts_with("old mode")
            || line.starts_with("new mode")
            || line.starts_with("Binary ")
        {
            continue;
        }
        if let Some(caps) = hunk_re.captures(line) {
            cur_new = caps[1].parse().unwrap_or(1);
            if let Some(cur) = &current {
                let fd = map.entry(cur.clone()).or_default();
                fd.hunks.push(DiffHunk {
                    new_start: cur_new,
                    new_end: cur_new.saturating_sub(1),
                    additions: 0,
                    deletions: 0,
                });
            }
            continue;
        }
        let cur = match &current {
            Some(c) => c,
            None => continue,
        };
        let fd = match map.get_mut(cur) {
            Some(fd) => fd,
            None => continue,
        };
        match line.as_bytes().first().copied() {
            Some(b'+') => {
                fd.additions += 1;
                if let Some(h) = fd.hunks.last_mut() {
                    h.additions += 1;
                    h.new_end = cur_new;
                }
                cur_new += 1;
            }
            Some(b'-') => {
                fd.deletions += 1;
                if let Some(h) = fd.hunks.last_mut() {
                    h.deletions += 1;
                }
            }
            Some(b'\\') => { /* "\ No newline at end of file" — ignore */ }
            _ => {
                // Context line (leading space) or a blank context line.
                if let Some(h) = fd.hunks.last_mut() {
                    h.new_end = cur_new;
                }
                cur_new += 1;
            }
        }
    }
    map
}

/// Strip a unified-diff path's `a/`/`b/` prefix AND any trailing tab
/// metadata, yielding the workspace-relative path. `/dev/null` (an
/// added/deleted side) yields `None`.
fn normalize_diff_path(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let raw = raw.split('\t').next().unwrap_or(raw);
    if raw == "/dev/null" {
        return None;
    }
    let stripped = raw
        .strip_prefix("b/")
        .or_else(|| raw.strip_prefix("a/"))
        .unwrap_or(raw);
    Some(stripped.to_string())
}

/// File extension (lowercased) for a workspace-relative path, or empty
/// when there is none.
fn file_extension(path: &str) -> String {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// Operator-facing verdict for the re-review entry point (a33). The
/// existing `ReviewVerdict::{Pass, Concerns}` both map to `Approve`;
/// `Block` stays `Block`. The two-state surface matches the spec's
/// `Verdict (Approve | Block)` contract for operator-initiated re-reviews.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Approve,
    Block,
}

impl Verdict {
    pub fn label(self) -> &'static str {
        match self {
            Verdict::Approve => "Approve",
            Verdict::Block => "Block",
        }
    }
}

impl From<ReviewVerdict> for Verdict {
    fn from(v: ReviewVerdict) -> Self {
        match v {
            ReviewVerdict::Block => Verdict::Block,
            ReviewVerdict::Pass | ReviewVerdict::Concerns => Verdict::Approve,
            // A failed-to-run review is NOT an approval. This conversion feeds
            // the operator re-review `Verdict {Approve, Block}` contract, which
            // a failed-to-run state should never reach in practice (a re-review
            // produces a real verdict); map it conservatively to Block rather
            // than waving it through as Approve.
            ReviewVerdict::FailedToRun => Verdict::Block,
        }
    }
}

/// Operator-facing per-concern record (a33). Mirrors [`ReviewConcern`] but
/// kept as a separate type so the operator-trigger entry point's public
/// surface does not bind to the LLM-output parsing struct.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConcernEntry {
    pub summary: String,
    pub actionable_request: Option<String>,
    pub should_request_revision: bool,
    pub change_slug: Option<String>,
}

impl From<&ReviewConcern> for ConcernEntry {
    fn from(c: &ReviewConcern) -> Self {
        Self {
            summary: c.summary.clone(),
            actionable_request: c.actionable_request.clone(),
            should_request_revision: c.should_request_revision,
            change_slug: c.change_slug.clone(),
        }
    }
}

/// Operator-facing review result (a33). Returned by
/// [`review_pr_at_state`]. Carries the verdict, per-concern records, the
/// rendered markdown body, AND the per-change sections (empty in
/// bundled mode).
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ReviewResult {
    pub verdict: Verdict,
    pub per_concern: Vec<ConcernEntry>,
    pub raw_output: String,
    pub markdown: String,
    pub per_change_sections: Vec<PerChangeSection>,
    pub concerns: Vec<ReviewConcern>,
    /// Redaction-safe `<provider>/<model>` attribution (a49), carried from
    /// the underlying [`ReviewReport`]. The rerun composer renders it as
    /// `*Reviewer: <provider>/<model>*` on the `## Code Review (rerun N of
    /// M)` comment. `None` when the reviewer carried no configured model.
    pub attribution: Option<String>,
}

/// Reusable reviewer entry point (a33 task 5). The polling-loop AND the
/// operator-trigger dispatcher both invoke this function; the caller
/// decides what to do with the returned `ReviewResult` (write into PR
/// body OR post as fresh PR comment).
///
/// The function:
/// - Builds a [`CodeReviewer`] from `cfg`.
/// - Performs the per-mode dispatch identically to the polling-loop's
///   initial-review path: one LLM call per change in `per_change` mode
///   (populating `ReviewResult.per_change_sections`), one call per PR in
///   `bundled` mode (leaving `per_change_sections` empty).
/// - Wraps the resulting report in a [`ReviewResult`].
///
/// Both callers therefore observe the configured `reviewer.mode`
/// identically; the function never routes through a bundled-only path
/// that ignores the mode (a53).
#[allow(dead_code)]
pub async fn review_pr_at_state(
    cfg: &ReviewerConfig,
    ctx: &ReviewContext,
) -> Result<ReviewResult> {
    let reviewer = CodeReviewer::from_config(cfg)?;
    review_pr_at_state_with(&reviewer, ctx).await
}

/// Test-friendly variant: dispatches against a caller-supplied
/// [`CodeReviewer`] so unit tests can stub the LLM client. The polling-
/// loop AND operator-trigger callers use [`review_pr_at_state`] which
/// builds the reviewer from config.
///
/// Dispatch honors `reviewer.mode()` (a53): in `Bundled` mode the single
/// `ReviewContext` is reviewed in one call (`per_change_sections` empty);
/// in `PerChange` mode the context is split into one per-change context
/// per `archived_changes` entry, each reviewed independently, and the
/// results are synthesized into a report carrying one
/// `per_change_sections` entry per change. The function decides nothing
/// about output disposition — the caller renders the returned
/// `ReviewResult`.
pub async fn review_pr_at_state_with(
    reviewer: &CodeReviewer,
    ctx: &ReviewContext,
) -> Result<ReviewResult> {
    let report = match reviewer.mode() {
        crate::config::ReviewerMode::Bundled => reviewer.review(ctx).await?,
        crate::config::ReviewerMode::PerChange => {
            let contexts = split_per_change_contexts(ctx);
            // a015: an empty split (no archived-change briefs resolved for
            // this PR — e.g. a PR opened under one daemon build and
            // re-reviewed under another) must NEVER synthesize a verdict
            // from zero reviews. `review_per_change(&[])` makes zero
            // reviewer invocations and `synthesize_per_change_report(vec![])`
            // would return a defaulted `Pass` — a blank `Approve` the
            // reviewer never performed. Fall back to a single bundled
            // review so the PR's diff and changed files still reach the
            // reviewer and the verdict reflects an actual invocation.
            if contexts.is_empty() {
                reviewer.review(ctx).await?
            } else {
                let per_change = reviewer.review_per_change(&contexts).await?;
                synthesize_per_change_report(per_change)
            }
        }
    };
    Ok(ReviewResult {
        verdict: Verdict::from(report.verdict),
        per_concern: report.concerns.iter().map(ConcernEntry::from).collect(),
        raw_output: report.markdown.clone(),
        markdown: report.markdown.clone(),
        per_change_sections: report.per_change_sections.clone(),
        attribution: report.attribution.clone(),
        concerns: report.concerns,
    })
}

/// Relative path (within the workspace) of the transient unified-diff
/// artifact the agentic reviewer reads on demand. Keyed by session slug so
/// per_change sessions don't collide; a bundled session uses `bundled`. The
/// daemon writes it before the read-only session AND removes it afterward.
fn review_diff_artifact_rel(slug: &str) -> String {
    let key = if slug.is_empty() { "bundled" } else { slug };
    format!(".autocoder-review-diff-{key}.patch")
}

/// Whether `cli` resolves to an executable on the daemon host. An absolute
/// or path-qualified command (`/usr/local/bin/claude`, `./claude`) is tested
/// directly; a bare name (`claude`) is searched across the entries in `$PATH`.
/// No subprocess is spawned — the binary is located, not executed — so the
/// startup probe is fast AND has no side effects. Used by
/// [`resolve_startup_reviewer_kind`] for the a64 agentic-CLI fallback.
fn reviewer_binary_on_path(cli: &str) -> bool {
    let candidate = Path::new(cli);
    if candidate.is_absolute() || cli.contains('/') {
        return candidate.is_file();
    }
    match std::env::var_os("PATH") {
        Some(path_var) => std::env::split_paths(&path_var).any(|dir| dir.join(cli).is_file()),
        None => false,
    }
}

/// Pure decision behind the a64 startup CLI-availability fallback. Given the
/// configured reviewer transport, the resolved CLI name, AND whether that CLI
/// is available on the host, return the effective startup transport plus an
/// optional loud WARN message:
///
/// - `Oneshot` configured → `(Oneshot, None)`: the operator opted out of
///   agentic deliberately, so no probe AND no warning.
/// - `Agentic` configured AND CLI available → `(Agentic, None)`: agentic runs.
/// - `Agentic` configured AND CLI unavailable → `(Oneshot, Some(warn))`: the
///   reviewer degrades to the HTTP one-shot path for the boot (review is NOT
///   disabled) AND the caller logs `warn`, which names the missing CLI AND the
///   remedy. The same disposition applies whether `agentic` was the default or
///   set explicitly.
///
/// Separated from the host probe ([`resolve_startup_reviewer_kind`]) so tests
/// assert the decision without depending on what is installed on the host —
/// mirroring [`crate::config::clamp_max_code_reviews_per_pr`]'s observable
/// `Option<String>` warning return.
pub fn startup_reviewer_kind_decision(
    configured: ReviewerKind,
    cli: &str,
    cli_available: bool,
) -> (ReviewerKind, Option<String>) {
    match configured {
        ReviewerKind::Oneshot => (ReviewerKind::Oneshot, None),
        ReviewerKind::Agentic if cli_available => (ReviewerKind::Agentic, None),
        ReviewerKind::Agentic => {
            let warn = format!(
                "reviewer.kind is `agentic` but the resolved reviewer CLI `{cli}` is unavailable \
                 on the daemon host (no registered strategy, OR the binary is not on PATH); \
                 falling back to the `oneshot` HTTP review path for this boot — review is NOT \
                 disabled. Install `{cli}` to enable the agentic reviewer, OR set \
                 `reviewer.kind: oneshot` to silence this warning. A daemon restart or \
                 `autocoder reload` re-evaluates availability."
            );
            (ReviewerKind::Oneshot, Some(warn))
        }
    }
}

/// Resolve the reviewer's effective transport at startup AND on
/// `autocoder reload`, applying the a64 agentic-CLI-availability fallback.
///
/// When the configured kind is `agentic` (defaulted OR explicit) this probes
/// the host: the CLI is "available" only when its strategy is registered
/// (resolved via the a55/a56 `provider → CLI` rule) AND its binary is found on
/// PATH. An unavailable CLI degrades to `oneshot` for the boot, returning the
/// loud WARN for the caller to log exactly once. When the configured kind is
/// `oneshot` no probe runs. The daemon wires this in at the two reviewer
/// construction sites (startup in `cli::run`, reload in `control_socket`), so
/// availability is evaluated once per boot/reload — never per polling
/// iteration. This supersedes a58's "a reviewer CLI with no registered
/// strategy returns a clear error, no session" behavior for the reviewer role:
/// instead of erroring, the reviewer degrades to HTTP review.
pub fn resolve_startup_reviewer_kind(reviewer: &CodeReviewer) -> (ReviewerKind, Option<String>) {
    if reviewer.kind() != ReviewerKind::Agentic {
        return (reviewer.kind(), None);
    }
    // "Available" requires BOTH a registered strategy AND a binary on PATH.
    let cli_available =
        resolve_reviewer_strategy(reviewer).is_ok() && reviewer_binary_on_path(&reviewer.command);
    startup_reviewer_kind_decision(ReviewerKind::Agentic, &reviewer.command, cli_available)
}

/// Apply the a64 startup CLI-availability fallback to a freshly built
/// reviewer. When the effective kind is `agentic` but the resolved reviewer
/// CLI is unavailable, log ONE loud WARN (naming the missing CLI AND the
/// remedy) AND return the reviewer with its kind overridden to `oneshot` for
/// the boot — review continues over HTTP, never disabled. Otherwise the
/// reviewer is returned unchanged. Both reviewer construction sites (startup
/// in `cli::run`, reload in `control_socket::build_reviewer`) call this, so
/// availability is evaluated once per boot/reload — the live polling-loop
/// reviewer slot already carries the resolved kind, so no per-iteration probe
/// (and no re-warn) occurs.
pub fn apply_startup_cli_fallback(reviewer: CodeReviewer) -> CodeReviewer {
    let (effective, warn) = resolve_startup_reviewer_kind(&reviewer);
    if let Some(msg) = warn {
        tracing::warn!("{msg}");
    }
    reviewer.with_kind(effective)
}

// =====================================================================
// On-demand review of a PR, commit, or target (a59)
// =====================================================================

/// The review SURFACE an on-demand review runs over (a59). Resolved by the
/// caller (control socket / CLI) from the operator's `<target>` argument:
/// a `pr`/`commit` resolves to a `Diff`; `files`/free-text resolves to a
/// [`ReviewTarget`]. The orchestration in [`run_on_demand_review`] turns the
/// surface into one or more [`ReviewContext`]s and runs the existing agentic
/// reviewer over each.
#[derive(Debug, Clone)]
pub enum ReviewSurface {
    /// A unified diff + its changed-file paths (a PR's base..head range OR a
    /// single commit's `git show`). Reviewed exactly like the per-pass diff.
    Diff {
        diff: String,
        changed_files: Vec<String>,
    },
    /// An on-demand TARGET review (a file-set OR a described area), carrying
    /// no diff.
    Target(ReviewTarget),
}

/// The operator's parsed `<target>` for an on-demand review (a59), before
/// resolution against the repository's local clone. Built by the chatops
/// verb / CLI subcommand parser; resolved into a [`ReviewSurface`] by
/// [`resolve_review_surface`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewTargetSpec {
    /// `pr <number>` — review the PR's base..head diff, resolved from the
    /// local clone.
    Pr { number: u64 },
    /// `commit <sha>` — review a single commit's diff (`git show`).
    Commit { sha: String },
    /// `files <path> [<path> ...]` — review the current content of named
    /// files (no diff).
    Files { paths: Vec<String> },
    /// A free-text description — the reviewer locates the files itself.
    Description { focus: String },
}

impl ReviewTargetSpec {
    /// Parse an operator `<target>` token list into a [`ReviewTargetSpec`].
    /// `pr <N>` / `commit <sha>` / `files <path...>` are recognized by their
    /// leading keyword; anything else is a free-text [`Description`] joining
    /// the tokens with single spaces. Returns `Err(reason)` for a malformed
    /// keyword form (e.g. `pr` with a non-numeric number, `files` with no
    /// paths) so the operator sees a usage error rather than a silent
    /// fall-through to a description.
    pub fn parse(tokens: &[String]) -> std::result::Result<Self, String> {
        let first = tokens.first().map(|s| s.as_str()).unwrap_or("");
        match first {
            "pr" => {
                let n = tokens.get(1).ok_or_else(|| {
                    "review pr: missing PR number. Usage: review <repo> pr <N>".to_string()
                })?;
                let number = n.parse::<u64>().map_err(|_| {
                    format!("review pr: `{n}` is not a valid PR number")
                })?;
                if tokens.len() > 2 {
                    return Err(
                        "review pr: too many arguments. Usage: review <repo> pr <N>".to_string(),
                    );
                }
                Ok(ReviewTargetSpec::Pr { number })
            }
            "commit" => {
                let sha = tokens.get(1).ok_or_else(|| {
                    "review commit: missing SHA. Usage: review <repo> commit <sha>".to_string()
                })?;
                if tokens.len() > 2 {
                    return Err(
                        "review commit: too many arguments. Usage: review <repo> commit <sha>"
                            .to_string(),
                    );
                }
                Ok(ReviewTargetSpec::Commit { sha: sha.clone() })
            }
            "files" => {
                let paths: Vec<String> = tokens[1..].to_vec();
                if paths.is_empty() {
                    return Err(
                        "review files: missing path(s). Usage: review <repo> files <path> [<path> ...]"
                            .to_string(),
                    );
                }
                Ok(ReviewTargetSpec::Files { paths })
            }
            "" => Err("review: missing target. Usage: review <repo> <pr N | commit SHA | files PATHS | description>".to_string()),
            _ => Ok(ReviewTargetSpec::Description {
                focus: tokens.join(" "),
            }),
        }
    }
}

/// Resolve a [`ReviewTargetSpec`] into a [`ReviewSurface`] against the
/// repository's local clone (a59). `pr <N>` fetches the PR head into a local
/// ref AND produces the base..head diff; `commit <sha>` produces that
/// commit's `git show` diff; `files` / a description produce TARGET surfaces
/// (no diff). `base_branch` is the repo's configured base, `remote` the
/// origin remote for PR-ref fetching. Network/git failures propagate as
/// `Err` for the caller to surface.
pub fn resolve_review_surface(
    spec: &ReviewTargetSpec,
    workspace: &Path,
    base_branch: &str,
    remote: &str,
) -> Result<ReviewSurface> {
    match spec {
        ReviewTargetSpec::Pr { number } => {
            let head_ref = crate::git::fetch_pull_request_head(workspace, remote, *number)?;
            let diff = crate::git::diff_three_dot(workspace, base_branch, &head_ref)?;
            let changed_files =
                crate::git::diff_files_changed(workspace, base_branch, &head_ref)?;
            Ok(ReviewSurface::Diff {
                diff,
                changed_files,
            })
        }
        ReviewTargetSpec::Commit { sha } => {
            let shas = vec![sha.clone()];
            let diff = crate::git::diff_for_commits(workspace, &shas)?;
            let changed_files = crate::git::files_for_commits(workspace, &shas)?;
            Ok(ReviewSurface::Diff {
                diff,
                changed_files,
            })
        }
        ReviewTargetSpec::Files { paths } => Ok(ReviewSurface::Target(ReviewTarget::Files {
            paths: paths.clone(),
        })),
        ReviewTargetSpec::Description { focus } => {
            Ok(ReviewSurface::Target(ReviewTarget::Description {
                focus: focus.clone(),
            }))
        }
    }
}

/// Default ceiling on files reviewed in ONE bounded session before the
/// orchestration chunks a target into multiple sessions (a59 scale clause).
/// A target at or under this count is reviewed in a single session; a larger
/// target is split (per file or per module) into multiple sessions whose
/// findings are aggregated into one report, so a broad area degrades into
/// bounded sessions rather than overflowing the model's context. Only
/// `Diff` and `Files` surfaces (whose file set is known up front) chunk; a
/// `Description` target is one session (the agent scopes itself via
/// Glob/Grep).
pub const ON_DEMAND_MAX_FILES_PER_SESSION: usize = 20;

/// One aggregated on-demand review report (a59). Carries the worst-of
/// verdict across all chunk sessions, the rendered body, the per-chunk
/// sections (one per session when chunked; one for a single bounded
/// session), AND the count of sessions run so the caller can tell the
/// operator what was chunked. Advisory + read-only — the caller reports it
/// and opens no revision.
#[derive(Debug, Clone)]
pub struct OnDemandReviewReport {
    pub verdict: Verdict,
    /// The aggregated review body (summary + concerns), already rendered for
    /// posting to chat / a PR comment.
    pub markdown: String,
    pub concerns: Vec<ReviewConcern>,
    /// Number of reviewer sessions that ran (1 for a bounded target, >1 when
    /// the target was chunked).
    pub sessions: usize,
    /// Per-chunk labels (file group or "all") in session order, for the
    /// "what was chunked" log AND the aggregated body header.
    pub chunk_labels: Vec<String>,
    pub attribution: Option<String>,
}

/// Split `files` into bounded chunks of at most `max_per_chunk` paths each
/// (a59 chunk-and-aggregate). A list at or under `max_per_chunk` yields a
/// single chunk; a larger list is split into consecutive groups. Pure for
/// testability. `max_per_chunk` is clamped to at least 1.
pub fn chunk_target_files(files: &[String], max_per_chunk: usize) -> Vec<Vec<String>> {
    let max = max_per_chunk.max(1);
    if files.len() <= max {
        return vec![files.to_vec()];
    }
    files.chunks(max).map(|c| c.to_vec()).collect()
}

/// Run an on-demand review over `surface` (a59). Production entry point for
/// the `review` chatops verb AND CLI subcommand. Resolves the reviewer's CLI
/// strategy (identical to [`run_agentic_review`]) then runs one or more
/// bounded agentic-reviewer sessions over the surface, chunking a large
/// target into multiple sessions AND aggregating their findings into one
/// report. Reuses the SAME agentic-reviewer machinery (sandbox,
/// `submit_review`, reads-on-demand) — it does NOT build a second reviewer.
///
/// A session that records no valid `submit_review` submission DISCARDS the
/// review (returns `AgenticReviewOutcome::Discarded`) rather than defaulting
/// to a clean pass, per the gatekeepers-fail-closed standard.
pub async fn run_on_demand_review(
    reviewer: &CodeReviewer,
    surface: &ReviewSurface,
    archived_changes: Vec<ChangeBrief>,
    workspace: &Path,
) -> Result<OnDemandReviewOutcome> {
    let strategy = resolve_reviewer_strategy(reviewer)?;
    let runner = CliReviewSessionRunner {
        workspace,
        strategy: strategy.as_ref(),
        cli: crate::config::default_cli_for(reviewer.provider),
        settings_dir: None,
        timeout: reviewer.agentic_session_timeout,
        model: reviewer.resolved_model.as_ref(),
    };
    run_on_demand_review_with_runner(reviewer, surface, archived_changes, &runner).await
}

/// Outcome of an on-demand review (a59). `Reviewed` carries the aggregated
/// [`OnDemandReviewReport`]; `Discarded` means at least one chunk session
/// produced no valid verdict, so the whole review is discarded (no
/// clean-pass default) AND the caller surfaces the failure.
#[derive(Debug, Clone)]
pub enum OnDemandReviewOutcome {
    Reviewed(OnDemandReviewReport),
    Discarded { reason: String },
}

/// Build the per-chunk [`ReviewContext`] list for a surface (a59):
/// - `Diff`: a single context carrying the diff + changed files when bounded;
///   when the changed-file count exceeds `max_per_chunk` the file list is
///   chunked AND each chunk still carries the FULL diff (the agent reads only
///   the chunk's files but has the diff for cross-reference). Each chunk's
///   label names the file group.
/// - `Files`: a `ReviewTarget::Files` per chunk (no diff), chunked the same way.
/// - `Description`: exactly one `ReviewTarget::Description` context — the agent
///   scopes the file set itself, so there is nothing to chunk up front.
fn build_on_demand_contexts(
    surface: &ReviewSurface,
    archived_changes: &[ChangeBrief],
    max_per_chunk: usize,
) -> Vec<(String, ReviewContext)> {
    match surface {
        ReviewSurface::Diff {
            diff,
            changed_files,
        } => {
            let chunks = chunk_target_files(changed_files, max_per_chunk);
            let single = chunks.len() <= 1;
            chunks
                .into_iter()
                .enumerate()
                .map(|(i, paths)| {
                    let label = if single {
                        "all".to_string()
                    } else {
                        format!("files {}-{}", i * max_per_chunk + 1, i * max_per_chunk + paths.len())
                    };
                    let ctx = ReviewContext {
                        archived_changes: archived_changes.to_vec(),
                        changed_files: paths
                            .into_iter()
                            .map(|path| ChangedFile {
                                path,
                                contents: String::new(),
                            })
                            .collect(),
                        diff: diff.clone(),
                        target: None,
                    };
                    (label, ctx)
                })
                .collect()
        }
        ReviewSurface::Target(ReviewTarget::Files { paths }) => {
            let chunks = chunk_target_files(paths, max_per_chunk);
            let single = chunks.len() <= 1;
            chunks
                .into_iter()
                .enumerate()
                .map(|(i, group)| {
                    let label = if single {
                        "all".to_string()
                    } else {
                        format!("files {}-{}", i * max_per_chunk + 1, i * max_per_chunk + group.len())
                    };
                    let ctx = ReviewContext {
                        archived_changes: archived_changes.to_vec(),
                        changed_files: Vec::new(),
                        diff: String::new(),
                        target: Some(ReviewTarget::Files { paths: group }),
                    };
                    (label, ctx)
                })
                .collect()
        }
        ReviewSurface::Target(ReviewTarget::Description { focus }) => {
            let ctx = ReviewContext {
                archived_changes: archived_changes.to_vec(),
                changed_files: Vec::new(),
                diff: String::new(),
                target: Some(ReviewTarget::Description {
                    focus: focus.clone(),
                }),
            };
            vec![("described area".to_string(), ctx)]
        }
    }
}

/// Mode-aware on-demand orchestration shared by production AND tests (a59).
/// Builds the per-chunk contexts (one bounded session, OR multiple chunked
/// sessions for a large file set), runs each through ONE agentic reviewer
/// session, AND aggregates their findings into a single
/// [`OnDemandReviewReport`]. Any chunk session that records no valid
/// submission discards the WHOLE review (returns `Discarded`) — never a
/// defaulted clean pass.
async fn run_on_demand_review_with_runner(
    reviewer: &CodeReviewer,
    surface: &ReviewSurface,
    archived_changes: Vec<ChangeBrief>,
    runner: &dyn ReviewSessionRunner,
) -> Result<OnDemandReviewOutcome> {
    let contexts =
        build_on_demand_contexts(surface, &archived_changes, ON_DEMAND_MAX_FILES_PER_SESSION);
    let chunked = contexts.len() > 1;
    if chunked {
        let labels: Vec<&str> = contexts.iter().map(|(l, _)| l.as_str()).collect();
        tracing::info!(
            sessions = contexts.len(),
            chunks = %labels.join(", "),
            "on-demand review: target spans more files than one bounded session; \
             chunking into multiple reviewer sessions"
        );
    }

    let mut chunk_labels: Vec<String> = Vec::with_capacity(contexts.len());
    let mut results: Vec<ReviewResult> = Vec::with_capacity(contexts.len());
    for (label, ctx) in &contexts {
        // Each chunk is one bounded reviewer session. The slug labels the
        // session (used for the per-session diff artifact key); a sanitized
        // chunk label keeps multi-session artifacts from colliding.
        let slug = sanitize_session_slug(label);
        let artifact_rel = review_diff_artifact_rel(&slug);
        let prompt = render_agentic_review_prompt(ctx, "", &artifact_rel);
        let consumed = runner.run_session(&slug, &prompt, &ctx.diff).await?;
        match consumed {
            None => {
                let reason = format!(
                    "on-demand reviewer session (chunk `{label}`) recorded no valid \
                     submit_review submission"
                );
                return Ok(OnDemandReviewOutcome::Discarded { reason });
            }
            Some(payload) => {
                let result = payload_to_review_result(&payload).map_err(|e| {
                    anyhow!("recorded submit_review payload failed re-validation: {e}")
                })?;
                chunk_labels.push(label.clone());
                results.push(result);
            }
        }
    }

    let report = aggregate_on_demand_results(results, chunk_labels, reviewer.attribution.clone());
    Ok(OnDemandReviewOutcome::Reviewed(report))
}

/// Aggregate per-chunk on-demand [`ReviewResult`]s into one
/// [`OnDemandReviewReport`] (a59). The aggregate verdict is `Block` when ANY
/// chunk blocked, else `Approve`; the body concatenates each chunk's summary
/// under a `## Chunk: <label>` heading when chunked (a single bounded session
/// renders just its body); the concerns vec is the union across chunks.
fn aggregate_on_demand_results(
    results: Vec<ReviewResult>,
    chunk_labels: Vec<String>,
    attribution: Option<String>,
) -> OnDemandReviewReport {
    let sessions = results.len();
    let mut verdict = Verdict::Approve;
    let mut concerns: Vec<ReviewConcern> = Vec::new();
    let mut body = String::new();
    let chunked = sessions > 1;
    for (idx, result) in results.into_iter().enumerate() {
        if matches!(result.verdict, Verdict::Block) {
            verdict = Verdict::Block;
        }
        for c in &result.concerns {
            concerns.push(c.clone());
        }
        if chunked {
            if !body.is_empty() {
                body.push_str("\n\n");
            }
            let label = chunk_labels.get(idx).map(String::as_str).unwrap_or("chunk");
            body.push_str(&format!(
                "## Chunk: {label} — {}\n\n{}",
                result.verdict.label(),
                result.markdown.trim()
            ));
        } else {
            body.push_str(result.markdown.trim());
        }
    }
    if body.trim().is_empty() {
        body.push_str("(no concerns)");
    }
    OnDemandReviewReport {
        verdict,
        markdown: body,
        concerns,
        sessions,
        chunk_labels,
        attribution,
    }
}

/// Sanitize a chunk label into a filesystem-safe session slug for the
/// per-session diff artifact key. Keeps alphanumerics, replaces every other
/// run with a single `-`. An empty result falls back to `chunk`.
fn sanitize_session_slug(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    let mut prev_dash = false;
    for ch in label.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "chunk".to_string()
    } else {
        trimmed
    }
}

/// Aggregate per-change agentic [`ReviewResult`]s into one result whose
/// `per_change_sections` drives the composer to emit one
/// `## Code Review: <slug>` section per change — the same disposition the
/// one-shot per-change path produces. The aggregate verdict is `Block` when
/// ANY change blocked, else `Approve`; the flat `concerns` vec is the union
/// of each change's concerns tagged with their `change_slug`.
fn synthesize_agentic_per_change(
    reviews: Vec<(Option<String>, ReviewResult)>,
    attribution: Option<String>,
) -> ReviewResult {
    // a015: a synthesis from zero per-change reviews must NEVER be the
    // source of a defaulted `Approve`. The dispatch in
    // `run_agentic_review_with_runner` now falls back to a bundled session
    // before reaching here with an empty vec, so this guard is defensive —
    // it makes the "never a defaulted Approve" invariant explicit. `Block`
    // is the only fail-safe verdict: an empty synthesis can never become a
    // silent approval. (Mirrors the one-shot `synthesize_per_change_report`
    // guard.)
    if reviews.is_empty() {
        return ReviewResult {
            verdict: Verdict::Block,
            per_concern: Vec::new(),
            raw_output: String::new(),
            markdown: "No per-change reviews were performed; refusing to \
                synthesize a verdict from zero reviews."
                .to_string(),
            per_change_sections: Vec::new(),
            concerns: Vec::new(),
            attribution,
        };
    }
    let mut verdict = Verdict::Approve;
    let mut concerns: Vec<ReviewConcern> = Vec::new();
    let mut sections: Vec<PerChangeSection> = Vec::with_capacity(reviews.len());
    for (slug, result) in reviews {
        let slug = slug.unwrap_or_default();
        if matches!(result.verdict, Verdict::Block) {
            verdict = Verdict::Block;
        }
        for concern in &result.concerns {
            let mut tagged = concern.clone();
            tagged.change_slug = Some(slug.clone());
            concerns.push(tagged);
        }
        let section_body = format!(
            "VERDICT: {}\n\n{}",
            result.verdict.label(),
            result.raw_output
        );
        sections.push(PerChangeSection {
            change_slug: slug,
            markdown: section_body,
        });
    }
    let per_concern = concerns.iter().map(ConcernEntry::from).collect();
    ReviewResult {
        verdict,
        per_concern,
        raw_output: String::new(),
        markdown: String::new(),
        per_change_sections: sections,
        concerns,
        attribution,
    }
}

/// Register the reviewer's `submit_review` payload schema (a58) with the
/// daemon's submission store, under [`REVIEWER_ROLE`]. The validator IS
/// [`payload_to_review_result`] with its `Ok` value discarded, so a
/// payload that records successfully is exactly one that maps. Called once
/// at daemon startup alongside the advisory audits' schema registration.
pub fn register_reviewer_submission_schema(store: &crate::submission_store::SubmissionStore) {
    use std::sync::Arc;
    store.register_schema(
        REVIEWER_ROLE,
        Arc::new(|p: &Value| payload_to_review_result(p).map(|_| ())),
    );
}

impl ReviewResult {
    /// Convert an agentic [`ReviewResult`] into the [`ReviewReport`] the
    /// polling-loop's post-review pipeline consumes (draft decision,
    /// reviewer-revision partitioning, PR-body composition). The two-state
    /// agentic verdict maps `Approve → Pass` AND `Block → Block`.
    pub fn into_review_report(self) -> ReviewReport {
        let verdict = match self.verdict {
            Verdict::Approve => ReviewVerdict::Pass,
            Verdict::Block => ReviewVerdict::Block,
        };
        ReviewReport {
            verdict,
            markdown: self.markdown,
            concerns: self.concerns,
            per_change_sections: self.per_change_sections,
            attribution: self.attribution,
        }
    }
}

/// Split a bundled [`ReviewContext`] into one [`PerChangeContext`] per
/// archived change, for the per-change reviewer dispatch on the reusable
/// entry point. Each per-change context carries that change's brief alone
/// plus a cross-change preamble naming the others; the changed-files AND
/// diff are shared across the per-change contexts. The single-
/// `ReviewContext` entry point has no per-change git scoping (unlike the
/// polling-loop path, which scopes each change's diff via commit-subject
/// prefixes), so the preamble is what confines each reviewer call's
/// verdict to its named change.
fn split_per_change_contexts(ctx: &ReviewContext) -> Vec<PerChangeContext> {
    ctx.archived_changes
        .iter()
        .map(|brief| PerChangeContext {
            change_slug: brief.name.clone(),
            context: ReviewContext {
                archived_changes: vec![brief.clone()],
                changed_files: ctx.changed_files.clone(),
                diff: ctx.diff.clone(),
                target: None,
            },
            cross_change_preamble: build_cross_change_preamble(&brief.name, &ctx.archived_changes),
        })
        .collect()
}

/// Aggregate a `Vec<PerChangeReview>` into one [`ReviewReport`] whose
/// `per_change_sections` drives the composer (PR-body or rerun comment)
/// to emit one `## Code Review: <slug>` section per element. The
/// aggregate `verdict` is the worst across sections (`Block` >
/// `Concerns` > `Pass`). The flat `concerns` vec is the union of each
/// per-change report's concerns (tagged with their `change_slug`), used
/// by the auto-revise pipeline.
pub(crate) fn synthesize_per_change_report(per_change: Vec<PerChangeReview>) -> ReviewReport {
    // a015: a synthesis from zero per-change reviews must NEVER be the
    // source of a defaulted `Pass`/`Approve`. The per_change dispatch arm
    // now falls back to a bundled review before reaching here with an
    // empty vec, so this guard is defensive — it makes that invariant
    // explicit. `Block` is the only verdict that does not map to `Approve`
    // on the operator-facing surface, so it is the fail-safe choice: an
    // empty synthesis can never become a silent approval.
    if per_change.is_empty() {
        return ReviewReport {
            verdict: ReviewVerdict::Block,
            markdown: "No per-change reviews were performed; refusing to \
                synthesize a verdict from zero reviews."
                .to_string(),
            concerns: Vec::new(),
            per_change_sections: Vec::new(),
            attribution: None,
        };
    }
    let mut verdict = ReviewVerdict::Pass;
    let mut concerns: Vec<ReviewConcern> = Vec::new();
    let mut sections: Vec<PerChangeSection> = Vec::with_capacity(per_change.len());
    // Every per-change report comes from the same reviewer, so they share
    // one attribution (a49); carry it onto the synthesized report so the
    // composer can attribute each `## Code Review: <slug>` section.
    let attribution = per_change
        .first()
        .and_then(|pcr| pcr.report.attribution.clone());
    for pcr in per_change {
        verdict = worst_verdict(verdict, pcr.report.verdict);
        for concern in &pcr.report.concerns {
            let mut tagged = concern.clone();
            tagged.change_slug = Some(pcr.change_slug.clone());
            concerns.push(tagged);
        }
        let section_body =
            format!("VERDICT: {}\n\n{}", verdict_label(pcr.report.verdict), pcr.report.markdown);
        sections.push(PerChangeSection {
            change_slug: pcr.change_slug,
            markdown: section_body,
        });
    }
    ReviewReport {
        verdict,
        markdown: String::new(),
        concerns,
        per_change_sections: sections,
        attribution,
    }
}

fn verdict_label(v: ReviewVerdict) -> &'static str {
    match v {
        ReviewVerdict::Pass => "Pass",
        ReviewVerdict::Concerns => "Concerns",
        ReviewVerdict::Block => "Block",
        ReviewVerdict::FailedToRun => "Failed to run",
    }
}

fn worst_verdict(a: ReviewVerdict, b: ReviewVerdict) -> ReviewVerdict {
    fn rank(v: ReviewVerdict) -> u8 {
        match v {
            ReviewVerdict::Pass => 0,
            ReviewVerdict::Concerns => 1,
            ReviewVerdict::Block => 2,
            // A failed-to-run review is non-passing; rank it above Block so
            // aggregating it with any other verdict never resolves to a pass.
            ReviewVerdict::FailedToRun => 3,
        }
    }
    if rank(a) >= rank(b) { a } else { b }
}

/// Emit a single INFO log line describing the rendered prompt's shape:
/// per-section bytes, per-file bytes, total vs. budget, and any files
/// dropped due to budget exhaustion. Operators rely on this to tell at a
/// glance whether a review approached the prompt-budget cap.
fn log_prompt_stats(
    ctx: &ReviewContext,
    rendered: &RenderedSections,
    prompt_bytes: usize,
    budget: usize,
) {
    let file_sizes: String = ctx
        .changed_files
        .iter()
        .map(|f| format!("{}:{}", f.path, f.contents.len()))
        .collect::<Vec<_>>()
        .join(",");
    let file_bytes_total: usize = ctx.changed_files.iter().map(|f| f.contents.len()).sum();
    let pct = prompt_bytes
        .saturating_mul(100)
        .checked_div(budget)
        .map(|p| p.min(999))
        .unwrap_or(0);
    tracing::info!(
        prompt_bytes = prompt_bytes,
        budget = budget,
        pct_of_budget = pct,
        change_context_bytes = rendered.change_context.len(),
        changed_files_bytes = rendered.changed_files.len(),
        diff_section_bytes = rendered.diff_or_explanation.len(),
        files_included = ctx.changed_files.len().saturating_sub(rendered.skipped_files.len()),
        files_skipped = rendered.skipped_files.len(),
        diff_input_bytes = ctx.diff.len(),
        file_count = ctx.changed_files.len(),
        file_content_total = file_bytes_total,
        skipped = %rendered.skipped_files.join(","),
        files = %file_sizes,
        "reviewer prompt built"
    );
}

/// Build the cross-change preamble for a per-change reviewer call:
/// names the OTHER changes in the same pass so the reviewer has cross-
/// reference context, while making clear the verdict applies only to
/// `this_change`.
///
/// Format (matches the spec's task 3.3 template):
/// ```text
/// This PR contains <N> changes. You are reviewing only `<slug>`.
/// Other changes in the same PR (for cross-reference context only — do not review them):
/// - <other-slug-1>: <other-1-summary>
/// - <other-slug-2>: <other-2-summary>
/// Your verdict applies ONLY to `<slug>`. The reviewer for each other change runs independently.
/// ```
///
/// Each `<other-summary>` is the first paragraph of the other change's
/// proposal `## Why` section, truncated to 200 chars. When the pass
/// contains a single change, the preamble is an empty string (no other
/// changes to reference).
pub fn build_cross_change_preamble(
    this_change: &str,
    all_changes: &[ChangeBrief],
) -> String {
    if all_changes.len() <= 1 {
        return String::new();
    }
    let n = all_changes.len();
    let mut out = format!(
        "This PR contains {n} changes. You are reviewing only `{this_change}`.\n\
         Other changes in the same PR (for cross-reference context only — do not review them):\n"
    );
    for brief in all_changes {
        if brief.name == this_change {
            continue;
        }
        let summary = first_paragraph_of_why(&brief.proposal);
        let truncated: String = summary.chars().take(200).collect();
        out.push_str(&format!("- {}: {}\n", brief.name, truncated));
    }
    out.push_str(&format!(
        "Your verdict applies ONLY to `{this_change}`. The reviewer for each other change runs independently.\n"
    ));
    out
}

/// Extract the first non-empty paragraph from a proposal's `## Why`
/// section. Returns an empty string when the section is absent or empty.
/// "Paragraph" = consecutive non-empty lines (joined with single spaces),
/// stopping at the first blank line or the next `## ` header.
fn first_paragraph_of_why(proposal: &str) -> String {
    let mut in_why = false;
    let mut paragraph_lines: Vec<&str> = Vec::new();
    for raw_line in proposal.lines() {
        let line = raw_line.trim_end();
        let trimmed = line.trim_start();
        if trimmed.starts_with("## ") {
            if in_why {
                break;
            }
            in_why = trimmed == "## Why";
            continue;
        }
        if !in_why {
            continue;
        }
        if line.trim().is_empty() {
            if !paragraph_lines.is_empty() {
                break;
            }
            continue;
        }
        paragraph_lines.push(line.trim());
    }
    paragraph_lines.join(" ")
}

/// Rendered substitution values for the three template placeholders, sized
/// against the configured `budget` in priority order. Pure function for
/// testability.
struct RenderedSections {
    change_context: String,
    changed_files: String,
    diff_or_explanation: String,
    /// Files whose contents were dropped to fit the budget. Empty when all
    /// files fit. Used by `review` to log a structured warning.
    skipped_files: Vec<String>,
}

fn render_sections(ctx: &ReviewContext, budget: usize) -> RenderedSections {
    // 1. Change context — always included in full. Change briefs are
    //    small (proposal/design/tasks of OpenSpec changes), so the
    //    worst-case overflow here would be a misuse anyway.
    let mut change_context = String::new();
    for brief in &ctx.archived_changes {
        if !change_context.is_empty() {
            change_context.push_str("\n\n");
        }
        change_context.push_str(&format!("## Change: {}\n\n", brief.name));
        change_context.push_str(brief.proposal.trim_end());
        if let Some(design) = brief.design.as_deref() {
            change_context.push_str("\n\n");
            change_context.push_str(design.trim_end());
        }
        change_context.push_str("\n\n");
        change_context.push_str(brief.tasks.trim_end());
    }

    // 2. Changed files — whole-file-or-skip against remaining budget.
    let mut changed_files = String::new();
    let mut skipped: Vec<String> = Vec::new();
    for file in &ctx.changed_files {
        // Approximate next-segment size: header + blank + body + trailing
        // separators. We don't need exact accounting; under-counting risks
        // pushing slightly past budget, over-counting drops files that
        // would have fit. Use a conservative additive estimate.
        let segment_len = file.path.len() + file.contents.len() + 64;
        let projected = change_context.len() + changed_files.len() + segment_len;
        if projected > budget {
            skipped.push(file.path.clone());
            continue;
        }
        if !changed_files.is_empty() {
            changed_files.push_str("\n\n");
        }
        changed_files.push_str(&format!("## File: {}\n\n", file.path));
        changed_files.push_str(&file.contents);
    }
    if !skipped.is_empty() {
        if !changed_files.is_empty() {
            changed_files.push_str("\n\n");
        }
        changed_files.push_str(&format!(
            "## Skipped (budget exhausted): {}",
            skipped.join(", ")
        ));
    }

    // 3. Diff — all-or-explanation. The diff is dropped if any files
    //    were skipped (the spec treats skipped files as the budget-
    //    exhaustion signal), OR if including the diff would push the
    //    rendered prompt past the configured budget.
    let used = change_context.len() + changed_files.len();
    let diff_or_explanation = if ctx.diff.is_empty() {
        String::from("(no diff produced this pass)")
    } else if !skipped.is_empty() || used + ctx.diff.len() > budget {
        String::from("(diff omitted: budget exhausted by change context and changed files)")
    } else {
        ctx.diff.clone()
    };

    RenderedSections {
        change_context,
        changed_files,
        diff_or_explanation,
        skipped_files: skipped,
    }
}

/// Whether the reviewer's own structured findings flag a security-critical
/// issue — a credential/secret/key exposure or an injection vulnerability —
/// via the per-concern `security_critical` signal (a004). This drives the
/// verdict-escalation safety net: such a finding forces a `Block` even when
/// the reviewer returned a softer verdict. It keys on the structured signal
/// the reviewer emitted, NEVER on the prose of the finding, so a
/// mis-classifying model cannot downgrade a credential leak to advisory and
/// a finding that merely mentions "credential" in passing does not escalate.
fn concerns_flag_security_critical(concerns: &[ReviewConcern]) -> bool {
    concerns.iter().any(|c| c.security_critical)
}

/// Parse the LLM response into a `ReviewReport`. Per spec, the first
/// non-empty line MUST match `(?i)^VERDICT:\s*(Pass|Concerns|Block)\s*$`.
/// If matched, the rest of the response (after that line) is the
/// `markdown`. If unmatched, the verdict defaults to `Concerns` and a
/// parse-failure note is prepended.
///
/// Additionally, a trailing fenced YAML block tagged
/// ```` ```revision-requests ```` is parsed (when present) into
/// `concerns`. The block is OPTIONAL — older reviewer templates that
/// have not been updated to emit it produce an empty `concerns` vec,
/// which keeps the reviewer-initiated revision flow inert.
fn parse_response(raw: &str) -> ReviewReport {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?i)^VERDICT:\s*(Pass|Concerns|Block)\s*$").unwrap());

    // Find the first non-empty line and try to parse a verdict from it.
    let mut lines = raw.lines();
    let mut found_idx: Option<usize> = None;
    let mut first_nonempty: Option<&str> = None;
    for (i, line) in raw.lines().enumerate() {
        if !line.trim().is_empty() {
            first_nonempty = Some(line.trim());
            found_idx = Some(i);
            break;
        }
    }

    let concerns = extract_revision_requests(raw);

    let mut report = match (first_nonempty, found_idx) {
        (Some(line), Some(idx)) if re.is_match(line) => {
            let caps = re.captures(line).unwrap();
            let verdict = match caps.get(1).unwrap().as_str().to_ascii_lowercase().as_str() {
                "pass" => ReviewVerdict::Pass,
                "concerns" => ReviewVerdict::Concerns,
                "block" => ReviewVerdict::Block,
                _ => unreachable!("regex group is alternation of three literals"),
            };
            // Skip the verdict line; the remainder is the markdown.
            let _ = lines.nth(idx); // advances past the verdict-line index
            let remainder: Vec<&str> = lines.collect();
            let markdown = remainder.join("\n").trim_start_matches('\n').to_string();
            ReviewReport {
                verdict,
                markdown,
                concerns,
                per_change_sections: Vec::new(),
                attribution: None,
            }
        }
        _ => ReviewReport {
            verdict: ReviewVerdict::Concerns,
            markdown: format!(
                "[reviewer response did not include a valid verdict line]\n\n{raw}"
            ),
            concerns,
            per_change_sections: Vec::new(),
            attribution: None,
        },
    };
    // a004 safety net: a review that flagged a credential/secret/key exposure
    // or injection (via the reviewer's own `security_critical` finding signal)
    // but returned a softer verdict is escalated to `Block` here — before the
    // PR-draft / auto-revise handling runs — so a mis-classifying model cannot
    // ship a security-critical finding through as advisory. Non-security
    // findings are untouched.
    if report.verdict != ReviewVerdict::Block
        && concerns_flag_security_critical(&report.concerns)
    {
        report.verdict = ReviewVerdict::Block;
    }
    report
}

/// Extract the `revision-requests` fenced YAML block from `raw` (if any)
/// and parse it into `Vec<ReviewConcern>`. A missing block, an unparseable
/// block, or one that doesn't deserialize to the expected shape all yield
/// an empty vec — the schema extension is opt-in for operator-customized
/// reviewer templates, so anything other than a well-formed block is
/// treated as "no concerns to act on" rather than an error.
fn extract_revision_requests(raw: &str) -> Vec<ReviewConcern> {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Match a fenced block opened with ``` (or ~~~) followed by
    // `revision-requests` (case-insensitive) as the info string, then any
    // body, then a closing fence on its own line. Multiline mode + dotall.
    let re = RE.get_or_init(|| {
        Regex::new(r"(?is)(?:^|\n)\s*```\s*revision-requests\s*\n(.*?)\n\s*```\s*(?:\n|$)")
            .expect("static regex compiles")
    });
    let body = match re.captures(raw).and_then(|c| c.get(1)) {
        Some(m) => m.as_str(),
        None => return Vec::new(),
    };
    match serde_yml::from_str::<Vec<ReviewConcern>>(body) {
        Ok(parsed) => parsed,
        Err(e) => {
            tracing::warn!(
                "failed to parse reviewer `revision-requests` YAML block: {e}; treating as no concerns"
            );
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::{Arc, Mutex};

    mod agentic;

    /// Test client that returns a pre-canned response and records the prompt
    /// it was asked to complete into a shared captured slot.
    struct StubClient {
        response: String,
        captured: Arc<Mutex<Option<String>>>,
    }
    #[async_trait]
    impl LlmClient for StubClient {
        async fn complete(&self, prompt: &str) -> anyhow::Result<String> {
            *self.captured.lock().unwrap() = Some(prompt.to_string());
            Ok(self.response.clone())
        }
    }

    /// Build a stub client + a handle to its capture slot. The handle stays
    /// valid as long as the test holds it (cloned `Arc`), independent of
    /// whether the client itself has been boxed into a `CodeReviewer`.
    fn stub_with_capture(response: &str) -> (Box<StubClient>, Arc<Mutex<Option<String>>>) {
        let captured: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let client = Box::new(StubClient {
            response: response.to_string(),
            captured: captured.clone(),
        });
        (client, captured)
    }

    #[test]
    fn parses_pass_verdict() {
        let r = parse_response("VERDICT: Pass\n\n## Security\n- None observed.\n");
        assert_eq!(r.verdict, ReviewVerdict::Pass);
        assert!(r.markdown.contains("## Security"));
        assert!(r.markdown.contains("None observed."));
        assert!(!r.markdown.contains("VERDICT:"), "verdict line must be stripped");
    }

    #[test]
    fn parses_block_verdict() {
        let r = parse_response("VERDICT: Block\n\nSQL injection in line 42.\n");
        assert_eq!(r.verdict, ReviewVerdict::Block);
        assert!(r.markdown.contains("SQL injection"));
    }

    #[test]
    fn case_insensitive_verdict() {
        let r = parse_response("verdict: concerns\n\nminor nit.\n");
        assert_eq!(r.verdict, ReviewVerdict::Concerns);
        let r = parse_response("VERDICT:   PASS   \n\nok\n");
        assert_eq!(r.verdict, ReviewVerdict::Pass);
        let r = parse_response("VeRdIcT: BLOCK\nbad\n");
        assert_eq!(r.verdict, ReviewVerdict::Block);
    }

    #[test]
    fn parses_revision_requests_block_with_full_fields() {
        let raw = r#"VERDICT: Block

## Possible bugs
- find_user drops the error context.
- log path is computed with the wrong base directory.

```revision-requests
- summary: "find_user drops the error context"
  actionable_request: "fix find_user to propagate the underlying error via anyhow::Context"
  should_request_revision: true
- summary: "log path uses the wrong base directory"
  actionable_request: "switch the base from workspace_root to log_dir in build_log_path"
  should_request_revision: true
```
"#;
        let r = parse_response(raw);
        assert_eq!(r.verdict, ReviewVerdict::Block);
        assert_eq!(r.concerns.len(), 2);
        assert_eq!(r.concerns[0].summary, "find_user drops the error context");
        assert_eq!(
            r.concerns[0].actionable_request.as_deref(),
            Some("fix find_user to propagate the underlying error via anyhow::Context")
        );
        assert!(r.concerns[0].should_request_revision);
        assert_eq!(r.concerns[1].summary, "log path uses the wrong base directory");
        assert!(r.concerns[1].should_request_revision);
    }

    #[test]
    fn missing_revision_requests_block_yields_empty_concerns() {
        // Older reviewer template that has not been updated to emit the
        // structured block — parse must succeed and produce an empty
        // concerns vec, so the auto-revise step finds nothing actionable.
        let raw = "VERDICT: Block\n\n## Summary\nproblems here.\n";
        let r = parse_response(raw);
        assert_eq!(r.verdict, ReviewVerdict::Block);
        assert!(r.concerns.is_empty());
    }

    #[test]
    fn revision_requests_block_with_missing_fields_uses_defaults() {
        // The block is well-formed YAML but the per-concern records omit
        // `actionable_request` and `should_request_revision`. Those fields
        // must default to None / false respectively.
        let raw = r#"VERDICT: Concerns

```revision-requests
- summary: "consider naming the helper better"
- summary: "another stylistic nit"
```
"#;
        let r = parse_response(raw);
        assert_eq!(r.verdict, ReviewVerdict::Concerns);
        assert_eq!(r.concerns.len(), 2);
        for c in &r.concerns {
            assert!(c.actionable_request.is_none());
            assert!(!c.should_request_revision);
        }
    }

    #[test]
    fn malformed_revision_requests_block_yields_empty_concerns() {
        let raw = r#"VERDICT: Block

```revision-requests
this is not yaml: at all: ::: {{{ broken
```
"#;
        let r = parse_response(raw);
        // Verdict parses cleanly; the broken block is treated as no
        // concerns rather than as a parse error.
        assert_eq!(r.verdict, ReviewVerdict::Block);
        assert!(r.concerns.is_empty());
    }

    #[test]
    fn revision_requests_extracted_even_when_verdict_unparseable() {
        // Unparseable verdict line falls through to the Concerns default
        // path. The concerns extraction is independent and should still
        // surface any well-formed block (so operators can debug their
        // template even when the verdict header is broken).
        let raw = r#"oops bad header

```revision-requests
- summary: "still gets through"
  should_request_revision: true
  actionable_request: "do the thing"
```
"#;
        let r = parse_response(raw);
        assert_eq!(r.verdict, ReviewVerdict::Concerns);
        assert_eq!(r.concerns.len(), 1);
        assert!(r.concerns[0].should_request_revision);
    }

    // =================================================================
    // a004: security-critical findings escalate the verdict to Block.
    // The escalation keys ONLY on the reviewer's own structured
    // `security_critical` signal, never on the prose of the finding.
    // =================================================================

    /// 3.1: a credential/secret-leak finding (`security_critical: true`)
    /// returned with a `Concerns` verdict is escalated to `Block`.
    #[test]
    fn security_finding_escalates_concerns_to_block() {
        let raw = r#"VERDICT: Concerns

## Security
- API key persisted to a committable config file.

```revision-requests
- summary: "API key written to committable opencode.json"
  actionable_request: "read the key from an env var instead of persisting it"
  should_request_revision: true
  security_critical: true
```
"#;
        let r = parse_response(raw);
        assert_eq!(
            r.verdict,
            ReviewVerdict::Block,
            "a security_critical finding must force Block even when the reviewer wrote Concerns"
        );
    }

    /// 3.1: the same escalation applies when the reviewer wrote `Pass`.
    #[test]
    fn security_finding_escalates_pass_to_block() {
        let raw = r#"VERDICT: Pass

## Security
- Token leaked into the workspace.

```revision-requests
- summary: "auth token written to a tracked file"
  security_critical: true
```
"#;
        let r = parse_response(raw);
        assert_eq!(r.verdict, ReviewVerdict::Block);
    }

    /// 3.2: an injection finding (also carried by `security_critical`) with
    /// a non-`Block` verdict escalates to `Block`.
    #[test]
    fn injection_finding_escalates_to_block() {
        let raw = r#"VERDICT: Concerns

## Security
- User input concatenated into a shell command.

```revision-requests
- summary: "command injection in run_hook"
  actionable_request: "pass arguments as a vector instead of building a shell string"
  should_request_revision: true
  security_critical: true
```
"#;
        let r = parse_response(raw);
        assert_eq!(r.verdict, ReviewVerdict::Block);
    }

    /// 3.3: a `Concerns` verdict whose findings are all non-security
    /// (`security_critical` omitted → `false`) stays `Concerns` — no
    /// escalation.
    #[test]
    fn non_security_concerns_are_not_escalated() {
        let raw = r#"VERDICT: Concerns

## Naming, style, idioms
- `tmp` is an unclear name.

```revision-requests
- summary: "rename tmp to something descriptive"
  should_request_revision: false
```
"#;
        let r = parse_response(raw);
        assert_eq!(
            r.verdict,
            ReviewVerdict::Concerns,
            "non-security findings must keep their verdict"
        );
        assert!(
            !r.concerns[0].security_critical,
            "omitted security_critical defaults to false"
        );
    }

    /// 3.4: the escalation is driven by the structured `security_critical`
    /// signal, NOT by message wording. A finding whose prose screams
    /// "credential leak" but is NOT flagged stays `Concerns`; an innocuous-
    /// worded finding that IS flagged escalates to `Block`.
    #[test]
    fn escalation_keys_on_signal_not_wording() {
        // Prose mentions a credential leak, but the structured signal is
        // absent (defaults to false) → no escalation.
        let worded_but_unflagged = r#"VERDICT: Concerns

```revision-requests
- summary: "possible credential leak / secret / api key exposure here"
  should_request_revision: false
```
"#;
        let r = parse_response(worded_but_unflagged);
        assert_eq!(
            r.verdict,
            ReviewVerdict::Concerns,
            "wording alone must NOT escalate — only the structured signal does"
        );

        // Innocuous wording, but the structured signal is set → escalates.
        let flagged_but_innocuous = r#"VERDICT: Concerns

```revision-requests
- summary: "tidy up helper foo"
  security_critical: true
```
"#;
        let r = parse_response(flagged_but_innocuous);
        assert_eq!(
            r.verdict,
            ReviewVerdict::Block,
            "the structured signal escalates regardless of innocuous wording"
        );
    }

    /// A `security_critical` finding that ALSO carries a `Block` verdict is
    /// a no-op for the escalation (already Block) — the verdict is unchanged.
    #[test]
    fn security_finding_already_block_is_unchanged() {
        let raw = r#"VERDICT: Block

```revision-requests
- summary: "hardcoded secret"
  security_critical: true
```
"#;
        let r = parse_response(raw);
        assert_eq!(r.verdict, ReviewVerdict::Block);
    }

    #[test]
    fn defaults_to_concerns_on_unparseable() {
        let raw = "I think this is fine, but maybe consider X. No verdict line at all.";
        let r = parse_response(raw);
        assert_eq!(r.verdict, ReviewVerdict::Concerns);
        assert!(r.markdown.contains("[reviewer response did not include a valid verdict line]"));
        assert!(r.markdown.contains(raw), "raw response must be preserved");
    }

    #[test]
    fn unparseable_when_verdict_value_invalid() {
        // Right shape but wrong verdict word — should fall through to Concerns default.
        let r = parse_response("VERDICT: LookGoodToMe\n\nfine\n");
        assert_eq!(r.verdict, ReviewVerdict::Concerns);
        assert!(r.markdown.contains("did not include a valid verdict line"));
    }

    #[test]
    fn unparseable_when_first_nonempty_line_is_not_verdict() {
        let r = parse_response("Some preamble.\n\nVERDICT: Pass\n");
        // Spec requires the first NON-EMPTY line to be the verdict line.
        assert_eq!(r.verdict, ReviewVerdict::Concerns);
    }

    fn ctx_with_diff(diff: &str) -> ReviewContext {
        ReviewContext {
            archived_changes: Vec::new(),
            changed_files: Vec::new(),
            diff: diff.to_string(),
            target: None,
        }
    }

    #[tokio::test]
    async fn substitutes_template_variables() {
        let (client, captured) = stub_with_capture("VERDICT: Pass\n");
        let template = "ctx={{change_context}}\nFILES<<<{{changed_files}}>>>\nDIFF<<<{{diff}}>>>"
            .to_string();
        let reviewer = CodeReviewer::new(client, template);
        let ctx = ReviewContext {
            archived_changes: vec![ChangeBrief {
                name: "demo".into(),
                proposal: "## Why\nfor reasons".into(),
                design: None,
                tasks: "- [x] do thing".into(),
            }],
            changed_files: vec![ChangedFile {
                path: "src/foo.rs".into(),
                contents: "fn foo() {}".into(),
            }],
            diff: "the diff content".into(),
            target: None,
        };
        reviewer.review(&ctx).await.unwrap();

        let prompt = captured.lock().unwrap().clone().unwrap();
        assert!(prompt.contains("ctx=## Change: demo"), "got: {prompt}");
        assert!(prompt.contains("FILES<<<## File: src/foo.rs"), "got: {prompt}");
        assert!(prompt.contains("fn foo() {}"));
        assert!(prompt.contains("DIFF<<<the diff content>>>"), "got: {prompt}");
    }

    #[tokio::test]
    async fn small_diff_is_passed_through_verbatim() {
        let small_diff = "x".repeat(100);
        let (client, captured) = stub_with_capture("VERDICT: Pass\n");
        let reviewer = CodeReviewer::new(client, "{{diff}}".to_string());
        reviewer.review(&ctx_with_diff(&small_diff)).await.unwrap();
        let prompt = captured.lock().unwrap().clone().unwrap();
        assert_eq!(prompt.matches('x').count(), 100);
        assert!(!prompt.contains("budget exhausted"));
    }

    /// Priority order: change context appears before changed files, which
    /// appear before the diff.
    #[tokio::test]
    async fn review_renders_change_context_before_files_before_diff() {
        let (client, captured) = stub_with_capture("VERDICT: Pass\n");
        let template = "{{change_context}}|{{changed_files}}|{{diff}}".to_string();
        let reviewer = CodeReviewer::new(client, template);
        let ctx = ReviewContext {
            archived_changes: vec![ChangeBrief {
                name: "alpha".into(),
                proposal: "PROP_SENTINEL".into(),
                design: None,
                tasks: "TASKS_SENTINEL".into(),
            }],
            changed_files: vec![ChangedFile {
                path: "src/a.rs".into(),
                contents: "FILE_SENTINEL".into(),
            }],
            diff: "DIFF_SENTINEL".into(),
            target: None,
        };
        reviewer.review(&ctx).await.unwrap();
        let prompt = captured.lock().unwrap().clone().unwrap();
        let prop_i = prompt.find("PROP_SENTINEL").expect("proposal present");
        let file_i = prompt.find("FILE_SENTINEL").expect("file present");
        let diff_i = prompt.find("DIFF_SENTINEL").expect("diff present");
        assert!(prop_i < file_i, "change context must precede files");
        assert!(file_i < diff_i, "files must precede diff");
    }

    /// Two files large enough to bust the budget together: the second one
    /// is skipped, listed in the skip footer, and the diff is replaced by
    /// the budget-exhausted explanation.
    #[tokio::test]
    async fn skips_files_when_budget_exhausts() {
        let (client, captured) = stub_with_capture("VERDICT: Pass\n");
        let template = "{{change_context}}|{{changed_files}}|{{diff}}".to_string();
        let reviewer = CodeReviewer::new(client, template);
        // Each file ~1.5MB; together they exceed the 2MB budget.
        let big = "y".repeat(1_500_000);
        let ctx = ReviewContext {
            archived_changes: Vec::new(),
            changed_files: vec![
                ChangedFile {
                    path: "first.rs".into(),
                    contents: big.clone(),
                },
                ChangedFile {
                    path: "second.rs".into(),
                    contents: big.clone(),
                },
            ],
            diff: "DIFF_SENTINEL".into(),
            target: None,
        };
        reviewer.review(&ctx).await.unwrap();
        let prompt = captured.lock().unwrap().clone().unwrap();
        assert!(prompt.contains("first.rs"), "first file must be present");
        assert!(
            prompt.contains("## Skipped (budget exhausted): second.rs"),
            "second file must be in skip list; got prompt of {} bytes",
            prompt.len()
        );
        assert!(
            prompt.contains("(diff omitted: budget exhausted by change context and changed files)"),
            "diff must be replaced by the budget-exhausted explanation"
        );
        assert!(
            !prompt.contains("DIFF_SENTINEL"),
            "actual diff must not appear when budget is exhausted"
        );
    }

    /// A single file larger than the whole budget: file is skipped in
    /// full (never partially included).
    #[tokio::test]
    async fn never_truncates_individual_file() {
        let (client, captured) = stub_with_capture("VERDICT: Pass\n");
        let reviewer = CodeReviewer::new(client, "{{changed_files}}".to_string());
        let huge = "z".repeat(DEFAULT_PROMPT_BUDGET + 100_000);
        let ctx = ReviewContext {
            archived_changes: Vec::new(),
            changed_files: vec![ChangedFile {
                path: "huge.rs".into(),
                contents: huge,
            }],
            diff: String::new(),
            target: None,
        };
        reviewer.review(&ctx).await.unwrap();
        let prompt = captured.lock().unwrap().clone().unwrap();
        // Either fully present or fully skipped — no partial slice. With
        // ~2.1MB content vs 2MB budget, we expect "skipped".
        assert!(
            prompt.contains("## Skipped (budget exhausted): huge.rs"),
            "huge file must be wholly skipped"
        );
        // The actual content (`zzz...`) must NOT have leaked into the
        // prompt — if it did, we'd see thousands of 'z' characters.
        let z_count = prompt.matches('z').count();
        assert_eq!(z_count, 0, "no partial file contents should leak into prompt");
    }

    /// Pure-function test for `render_sections`: verifies priority order
    /// and skip-list behavior without needing a stub LLM client.
    #[test]
    fn render_sections_priority_order_pure() {
        let ctx = ReviewContext {
            archived_changes: vec![ChangeBrief {
                name: "x".into(),
                proposal: "P".into(),
                design: Some("D".into()),
                tasks: "T".into(),
            }],
            changed_files: vec![ChangedFile {
                path: "a.rs".into(),
                contents: "BODY".into(),
            }],
            diff: "DELTA".into(),
            target: None,
        };
        let r = render_sections(&ctx, DEFAULT_PROMPT_BUDGET);
        assert!(r.change_context.contains("## Change: x"));
        assert!(r.change_context.contains("P\n\nD\n\nT"));
        assert!(r.changed_files.contains("## File: a.rs"));
        assert!(r.changed_files.contains("BODY"));
        assert_eq!(r.diff_or_explanation, "DELTA");
    }

    // ====================================================================
    // a67: advisory size flag (tasks 7.x / 8.7)
    // ====================================================================

    async fn review_with_size_thresholds(
        ctx: &ReviewContext,
        file_t: u64,
        func_t: u64,
    ) -> ReviewReport {
        let (client, _captured) = stub_with_capture("VERDICT: Pass\n\n## Review\nlooks fine\n");
        let reviewer =
            CodeReviewer::new(client, "{{diff}}".to_string()).with_size_thresholds(file_t, func_t);
        reviewer.review(ctx).await.unwrap()
    }

    /// 8.7a — a pass that grows a changed file past the file threshold
    /// yields a size advisory naming the file, AND leaves the verdict
    /// untouched.
    #[tokio::test]
    async fn size_advisory_flags_file_grown_past_threshold() {
        let contents: String = (0..60).map(|i| format!("// line {i}\n")).collect();
        let mut diff = String::from(
            "diff --git a/src/foo.rs b/src/foo.rs\n--- a/src/foo.rs\n+++ b/src/foo.rs\n@@ -1,1 +1,11 @@\n // line 0\n",
        );
        for i in 0..10 {
            diff.push_str(&format!("+// added {i}\n"));
        }
        let ctx = ReviewContext {
            archived_changes: Vec::new(),
            changed_files: vec![ChangedFile {
                path: "src/foo.rs".into(),
                contents,
            }],
            diff,
            target: None,
        };
        let report = review_with_size_thresholds(&ctx, 50, 20).await;
        assert!(
            report.markdown.contains("## Size advisory") && report.markdown.contains("src/foo.rs"),
            "expected a file size advisory: {}",
            report.markdown
        );
        // Size is advisory only — the parsed verdict is unchanged.
        assert_eq!(report.verdict, ReviewVerdict::Pass);
    }

    /// 8.7b — a pass that only shrinks an over-threshold file is NOT
    /// flagged.
    #[tokio::test]
    async fn size_advisory_skips_file_only_shrunk() {
        let contents: String = (0..60).map(|i| format!("// line {i}\n")).collect();
        let diff = String::from(
            "diff --git a/src/bar.rs b/src/bar.rs\n--- a/src/bar.rs\n+++ b/src/bar.rs\n@@ -1,5 +1,1 @@\n // keep\n-// del 0\n-// del 1\n-// del 2\n-// del 3\n",
        );
        let ctx = ReviewContext {
            archived_changes: Vec::new(),
            changed_files: vec![ChangedFile {
                path: "src/bar.rs".into(),
                contents,
            }],
            diff,
            target: None,
        };
        let report = review_with_size_thresholds(&ctx, 50, 20).await;
        assert!(
            !report.markdown.contains("## Size advisory"),
            "a shrinking pass must not be flagged: {}",
            report.markdown
        );
    }

    /// 8.7c — a pass that grows a single function past the function
    /// threshold yields a function-level advisory.
    #[tokio::test]
    async fn size_advisory_flags_function_grown_past_threshold() {
        // 27-line function (1 signature + 25 body + 1 close).
        let mut contents = String::from("pub fn grower() {\n");
        for i in 0..25 {
            contents.push_str(&format!("    let v{i} = {i};\n"));
        }
        contents.push_str("}\n");
        // Diff that adds the 25 body lines (net +25 within the function).
        let mut diff = String::from(
            "diff --git a/src/grow.rs b/src/grow.rs\n--- a/src/grow.rs\n+++ b/src/grow.rs\n@@ -1,2 +1,27 @@\n pub fn grower() {\n",
        );
        for i in 0..25 {
            diff.push_str(&format!("+    let v{i} = {i};\n"));
        }
        diff.push_str(" }\n");
        let ctx = ReviewContext {
            archived_changes: Vec::new(),
            changed_files: vec![ChangedFile {
                path: "src/grow.rs".into(),
                contents,
            }],
            diff,
            target: None,
        };
        // High file threshold so only the function advisory can fire.
        let report = review_with_size_thresholds(&ctx, 100_000, 20).await;
        assert!(
            report.markdown.contains("## Size advisory") && report.markdown.contains("grower"),
            "expected a function size advisory naming `grower`: {}",
            report.markdown
        );
        assert_eq!(report.verdict, ReviewVerdict::Pass);
    }

    /// a34 §6: `skip_spec_only_prs` defaults to `false` AND propagates
    /// from `ReviewerConfig` via `from_config`. This is the gate the
    /// polling iteration consults before invoking the reviewer call.
    #[test]
    fn skip_spec_only_prs_defaults_false_and_propagates() {
        use crate::config::{ReviewerConfig, ReviewerProvider};
        // Default: unset → false.
        unsafe { std::env::set_var("REVIEWER_TEST_SKIP_DEFAULT", "k") };
        let cfg_default = ReviewerConfig {
            enabled: true,
            provider: Some(ReviewerProvider::Anthropic),
            model: "x".into(),
            api_key_env: Some("REVIEWER_TEST_SKIP_DEFAULT".into()),
            api_key: None,
            api_base_url: None,
            prompt_template_path: None,
            code_review: None,
            auto_revise: crate::config::AutoRevise::Off,
            prompt_budget_chars: 2_000_000,
            mode: crate::config::ReviewerMode::Bundled,
            max_code_reviews_per_pr: Some(5),
            suggest_rereview_threshold: None,
            skip_spec_only_prs: false,
            kind: crate::config::ReviewerKind::Oneshot,
            command: "claude".to_string(),
        };
        let r_default = CodeReviewer::from_config(&cfg_default)
            .expect("default-config builds");
        assert!(
            !r_default.skip_spec_only_prs(),
            "default must be false: got {}",
            r_default.skip_spec_only_prs()
        );

        // Explicit true: propagates.
        unsafe { std::env::set_var("REVIEWER_TEST_SKIP_TRUE", "k") };
        let cfg_true = ReviewerConfig {
            enabled: true,
            provider: Some(ReviewerProvider::Anthropic),
            model: "x".into(),
            api_key_env: Some("REVIEWER_TEST_SKIP_TRUE".into()),
            api_key: None,
            api_base_url: None,
            prompt_template_path: None,
            code_review: None,
            auto_revise: crate::config::AutoRevise::Off,
            prompt_budget_chars: 2_000_000,
            mode: crate::config::ReviewerMode::Bundled,
            max_code_reviews_per_pr: Some(5),
            suggest_rereview_threshold: None,
            skip_spec_only_prs: true,
            kind: crate::config::ReviewerKind::Oneshot,
            command: "claude".to_string(),
        };
        let r_true = CodeReviewer::from_config(&cfg_true)
            .expect("skip=true builds");
        assert!(
            r_true.skip_spec_only_prs(),
            "true must propagate: got {}",
            r_true.skip_spec_only_prs()
        );
        unsafe { std::env::remove_var("REVIEWER_TEST_SKIP_DEFAULT") };
        unsafe { std::env::remove_var("REVIEWER_TEST_SKIP_TRUE") };
    }

    /// a34 §6.2: a brownfield iteration's PR has only
    /// `openspec/changes/<change>/...` diff → `diff_is_spec_only`
    /// returns true. With `skip_spec_only_prs: true` the polling
    /// iteration's gate skips the reviewer call (verified via the
    /// predicate the polling code consults).
    #[test]
    fn diff_is_spec_only_classifies_brownfield_pr_correctly() {
        use crate::spec_storage_routing::diff_is_spec_only;
        let brownfield_paths = vec![
            "openspec/changes/a36-brownfield-foo/proposal.md".to_string(),
            "openspec/changes/a36-brownfield-foo/tasks.md".to_string(),
            "openspec/changes/a36-brownfield-foo/specs/foo/spec.md".to_string(),
        ];
        assert!(
            diff_is_spec_only(&brownfield_paths),
            "brownfield PR classifies as spec-only"
        );
    }

    /// a34 §6.3: a dual-tree iteration's code PR has
    /// `autocoder/src/foo.rs` diff → `diff_is_spec_only` returns
    /// false. The polling iteration's gate runs the reviewer normally.
    #[test]
    fn diff_is_spec_only_classifies_dual_tree_code_pr_correctly() {
        use crate::spec_storage_routing::diff_is_spec_only;
        let dual_code_paths = vec![
            "autocoder/src/foo.rs".to_string(),
            "openspec/changes/a36/proposal.md".to_string(),
        ];
        assert!(
            !diff_is_spec_only(&dual_code_paths),
            "dual-tree's code PR is NOT spec-only"
        );
    }

    /// a34 §6.4 (default behavior): with `skip_spec_only_prs: false`,
    /// the reviewer would be invoked even on a spec-only diff. The
    /// accessor returns false → the gate condition evaluates to false →
    /// the reviewer-invocation branch is taken.
    #[test]
    fn skip_spec_only_prs_false_does_not_short_circuit_gate() {
        use crate::config::{ReviewerConfig, ReviewerProvider};
        unsafe { std::env::set_var("REVIEWER_TEST_SKIP_FALSE_GATE", "k") };
        let cfg = ReviewerConfig {
            enabled: true,
            provider: Some(ReviewerProvider::Anthropic),
            model: "x".into(),
            api_key_env: Some("REVIEWER_TEST_SKIP_FALSE_GATE".into()),
            api_key: None,
            api_base_url: None,
            prompt_template_path: None,
            code_review: None,
            auto_revise: crate::config::AutoRevise::Off,
            prompt_budget_chars: 2_000_000,
            mode: crate::config::ReviewerMode::Bundled,
            max_code_reviews_per_pr: Some(5),
            suggest_rereview_threshold: None,
            skip_spec_only_prs: false,
            kind: crate::config::ReviewerKind::Oneshot,
            command: "claude".to_string(),
        };
        let r = CodeReviewer::from_config(&cfg).expect("config builds");
        unsafe { std::env::remove_var("REVIEWER_TEST_SKIP_FALSE_GATE") };
        // The polling-loop gate evaluates `r.skip_spec_only_prs() &&
        // diff_is_spec_only(...)`. When the first conjunct is false,
        // the gate is unconditionally false — the reviewer is invoked.
        assert!(
            !r.skip_spec_only_prs(),
            "default-false config keeps the gate inactive"
        );
    }

    #[test]
    fn from_config_reads_user_provided_template() {
        use crate::config::{ReviewerConfig, ReviewerProvider};
        let dir = tempfile::TempDir::new().unwrap();
        let template_path = dir.path().join("custom.md");
        std::fs::write(&template_path, "CUSTOM TEMPLATE: {{diff}}").unwrap();

        // Set the env var the config will read.
        unsafe { std::env::set_var("REVIEWER_TEST_KEY_OVERRIDE", "k") };
        let cfg = ReviewerConfig {
            enabled: true,
            provider: Some(ReviewerProvider::Anthropic),
            model: "x".into(),
            api_key_env: Some("REVIEWER_TEST_KEY_OVERRIDE".into()),
            api_key: None,
            api_base_url: None,
            prompt_template_path: Some(template_path),
            code_review: None,
            auto_revise: crate::config::AutoRevise::Off,
            prompt_budget_chars: 2_000_000,
            mode: crate::config::ReviewerMode::Bundled,
            max_code_reviews_per_pr: Some(5),
            suggest_rereview_threshold: None,
            skip_spec_only_prs: false,
            kind: crate::config::ReviewerKind::Oneshot,
            command: "claude".to_string(),
        };
        let reviewer = CodeReviewer::from_config(&cfg).expect("should load custom template");
        unsafe { std::env::remove_var("REVIEWER_TEST_KEY_OVERRIDE") };

        // Template identity, not wording: the loaded value is exactly the
        // synthetic custom template this test wrote, AND is distinct from
        // the embedded default (symbol comparison, no prose substring).
        assert_eq!(
            reviewer.template, "CUSTOM TEMPLATE: {{diff}}",
            "loaded template must equal the synthetic custom template the test wrote"
        );
        assert_ne!(
            reviewer.template, DEFAULT_TEMPLATE,
            "loaded custom template must not equal the embedded default"
        );
    }

    /// A missing prompt-template override path now falls back to the
    /// embedded default via the uniform `PromptLoader` (a24). The
    /// daemon does NOT abort start-up; instead a one-shot WARN names
    /// the offending path.
    #[test]
    fn from_config_falls_back_when_template_path_missing() {
        use crate::config::{ReviewerConfig, ReviewerProvider};
        unsafe { std::env::set_var("REVIEWER_TEST_KEY_MISSING_TMPL", "k") };
        let bogus = std::path::PathBuf::from("/nonexistent/orchestrator-test-template.md");
        let cfg = ReviewerConfig {
            enabled: true,
            provider: Some(ReviewerProvider::Anthropic),
            model: "x".into(),
            api_key_env: Some("REVIEWER_TEST_KEY_MISSING_TMPL".into()),
            api_key: None,
            api_base_url: None,
            prompt_template_path: Some(bogus.clone()),
            code_review: None,
            auto_revise: crate::config::AutoRevise::Off,
            prompt_budget_chars: 2_000_000,
            mode: crate::config::ReviewerMode::Bundled,
            max_code_reviews_per_pr: Some(5),
            suggest_rereview_threshold: None,
            skip_spec_only_prs: false,
            kind: crate::config::ReviewerKind::Oneshot,
            command: "claude".to_string(),
        };
        let reviewer = CodeReviewer::from_config(&cfg)
            .expect("missing template must fall back to embedded default");
        unsafe { std::env::remove_var("REVIEWER_TEST_KEY_MISSING_TMPL") };
        assert_eq!(
            reviewer.template, DEFAULT_TEMPLATE,
            "fallback must use the embedded default template (symbol identity)"
        );
    }

    /// The new `reviewer.code_review.prompt_path` nested form takes
    /// precedence over the legacy flat `reviewer.prompt_template_path`
    /// when both are set AND the nested file exists (a24).
    #[test]
    fn from_config_nested_form_preempts_legacy_for_reviewer() {
        use crate::config::{
            PromptOverrideBlock, ReviewerConfig, ReviewerProvider,
        };
        use tempfile::TempDir;
        unsafe { std::env::set_var("REVIEWER_TEST_KEY_NESTED", "k") };
        let tmp = TempDir::new().unwrap();
        let nested = tmp.path().join("nested-review.md");
        let legacy = tmp.path().join("legacy-review.md");
        std::fs::write(&nested, "NESTED REVIEW TEMPLATE").unwrap();
        std::fs::write(&legacy, "LEGACY REVIEW TEMPLATE").unwrap();
        let cfg = ReviewerConfig {
            enabled: true,
            provider: Some(ReviewerProvider::Anthropic),
            model: "x".into(),
            api_key_env: Some("REVIEWER_TEST_KEY_NESTED".into()),
            api_key: None,
            api_base_url: None,
            prompt_template_path: Some(legacy),
            code_review: Some(PromptOverrideBlock {
                prompt_path: Some(nested),
            }),
            auto_revise: crate::config::AutoRevise::Off,
            prompt_budget_chars: 2_000_000,
            mode: crate::config::ReviewerMode::Bundled,
            max_code_reviews_per_pr: Some(5),
            suggest_rereview_threshold: None,
            skip_spec_only_prs: false,
            kind: crate::config::ReviewerKind::Oneshot,
            command: "claude".to_string(),
        };
        let reviewer = CodeReviewer::from_config(&cfg)
            .expect("nested override resolves");
        unsafe { std::env::remove_var("REVIEWER_TEST_KEY_NESTED") };
        assert!(reviewer.template.contains("NESTED REVIEW TEMPLATE"));
        assert!(!reviewer.template.contains("LEGACY REVIEW TEMPLATE"));
    }

    #[test]
    fn from_config_uses_default_template_when_path_omitted() {
        use crate::config::{ReviewerConfig, ReviewerProvider};
        unsafe { std::env::set_var("REVIEWER_TEST_KEY_DEFAULT", "k") };
        let cfg = ReviewerConfig {
            enabled: true,
            provider: Some(ReviewerProvider::Anthropic),
            model: "x".into(),
            api_key_env: Some("REVIEWER_TEST_KEY_DEFAULT".into()),
            api_key: None,
            api_base_url: None,
            prompt_template_path: None,
            code_review: None,
            auto_revise: crate::config::AutoRevise::Off,
            prompt_budget_chars: 2_000_000,
            mode: crate::config::ReviewerMode::Bundled,
            max_code_reviews_per_pr: Some(5),
            suggest_rereview_threshold: None,
            skip_spec_only_prs: false,
            kind: crate::config::ReviewerKind::Oneshot,
            command: "claude".to_string(),
        };
        let reviewer = CodeReviewer::from_config(&cfg).expect("default template loads");
        unsafe { std::env::remove_var("REVIEWER_TEST_KEY_DEFAULT") };
        assert_eq!(
            reviewer.template, DEFAULT_TEMPLATE,
            "default template must be used when prompt_template_path is None (symbol identity)"
        );
    }

    /// The bug fix: `from_config` resolves the reviewer's model (exactly like
    /// the verifier gates) AND threads it onto the reviewer, so the agentic
    /// session passes `model: Some(_)` to the wrapped CLI — running the
    /// OPERATOR-configured model, not the CLI's own default. A KEYLESS
    /// `openai_compatible` reviewer resolves to an EMPTY key (opencode's keyless
    /// path) AND carries the operator's real opencode id verbatim.
    #[test]
    fn from_config_threads_keyless_resolved_model_onto_reviewer() {
        use crate::config::{ReviewerConfig, ReviewerProvider};
        let cfg = ReviewerConfig {
            enabled: true,
            provider: Some(ReviewerProvider::OpenAiCompatible),
            model: "openrouter/qwen/qwen3-max".into(),
            api_key_env: None,
            api_key: None,
            api_base_url: Some("https://api.example.invalid/v1".into()),
            prompt_template_path: None,
            code_review: None,
            auto_revise: crate::config::AutoRevise::Off,
            prompt_budget_chars: 2_000_000,
            mode: crate::config::ReviewerMode::Bundled,
            max_code_reviews_per_pr: None,
            suggest_rereview_threshold: None,
            skip_spec_only_prs: false,
            kind: crate::config::ReviewerKind::Agentic,
            command: "opencode".to_string(),
        };
        let reviewer = CodeReviewer::from_config(&cfg).expect("agentic reviewer builds");
        let m = reviewer
            .resolved_model
            .as_ref()
            .expect("from_config must resolve and thread the reviewer model (no more model: None)");
        assert_eq!(m.provider, LlmProvider::OpenAiCompatible);
        // Verbatim opencode id — fed to `--model` unchanged on the keyless path.
        assert_eq!(m.model, "openrouter/qwen/qwen3-max");
        assert!(
            m.api_key.is_empty(),
            "keyless reviewer → empty key so opencode takes its keyless path"
        );
    }

    /// unified-agentic-session-timeout task 4.1: a reviewer built from
    /// `ReviewerConfig` (which carries NO executor block) defaults its agentic
    /// session timeout to the resolved one-hour default — never a role-private
    /// literal.
    #[test]
    fn reviewer_defaults_agentic_session_timeout_to_one_hour() {
        let (client, _captured) = stub_with_capture("VERDICT: Pass\n");
        let r = CodeReviewer::new(client, "T".into());
        assert_eq!(
            r.agentic_session_timeout(),
            Duration::from_secs(crate::config::default_agentic_session_timeout()),
            "the reviewer's default must be the single resolved default (3600s)"
        );
    }

    /// unified-agentic-session-timeout task 4.2 (reviewer): the reviewer adopts
    /// the value resolved from `executor.agentic_session_timeout_secs` — the
    /// SAME single source the verifier gates AND the revision sessions use —
    /// rather than any reviewer-local constant. This is the config-to-call-site
    /// wiring both production reviewer construction sites perform.
    #[test]
    fn reviewer_adopts_configured_agentic_session_timeout() {
        let exec: crate::config::ExecutorConfig =
            serde_yml::from_str("kind: claude_cli\nagentic_session_timeout_secs: 5400\n")
                .expect("executor parses");
        let (client, _captured) = stub_with_capture("VERDICT: Pass\n");
        let r = CodeReviewer::new(client, "T".into())
            .with_agentic_session_timeout(exec.agentic_session_timeout());
        assert_eq!(
            r.agentic_session_timeout(),
            exec.agentic_session_timeout(),
            "the reviewer must use the resolved executor value, not a literal"
        );
        assert_eq!(r.agentic_session_timeout(), Duration::from_secs(5400));
    }

    /// A KEYED `openai_compatible` reviewer threads its resolved key into the
    /// reviewer's model (NON-empty), which drives the opencode strategy's keyed
    /// path (provider block + `<provider>/<model>` selection).
    #[test]
    fn from_config_threads_keyed_resolved_model_onto_reviewer() {
        use crate::config::{ReviewerConfig, ReviewerProvider, SecretSource};
        let cfg = ReviewerConfig {
            enabled: true,
            provider: Some(ReviewerProvider::OpenAiCompatible),
            model: "qwen3-max".into(),
            api_key_env: None,
            api_key: Some(SecretSource::Inline {
                value: "sk-reviewer-secret".into(),
            }),
            api_base_url: Some("https://api.example.invalid/v1".into()),
            prompt_template_path: None,
            code_review: None,
            auto_revise: crate::config::AutoRevise::Off,
            prompt_budget_chars: 2_000_000,
            mode: crate::config::ReviewerMode::Bundled,
            max_code_reviews_per_pr: None,
            suggest_rereview_threshold: None,
            skip_spec_only_prs: false,
            kind: crate::config::ReviewerKind::Agentic,
            command: "opencode".to_string(),
        };
        let reviewer = CodeReviewer::from_config(&cfg).expect("keyed agentic reviewer builds");
        let m = reviewer
            .resolved_model
            .as_ref()
            .expect("from_config must thread the keyed reviewer model");
        assert_eq!(
            m.api_key, "sk-reviewer-secret",
            "keyed reviewer threads its key (opencode keyed path: block + <provider>/<model>)"
        );
    }

    /// A higher prompt-budget cap lets a file through that the default
    /// cap would have skipped. Demonstrates the field is data-driven.
    #[tokio::test]
    async fn higher_prompt_budget_admits_files_default_would_skip() {
        let (client, captured) = stub_with_capture("VERDICT: Pass\n");
        let reviewer = CodeReviewer::new(client, "{{changed_files}}".to_string())
            .with_prompt_budget(4_000_000);
        // 3MB file: the default 2MB cap would skip it; the 4MB cap admits it.
        let three_mb = "y".repeat(3_000_000);
        let ctx = ReviewContext {
            archived_changes: Vec::new(),
            changed_files: vec![ChangedFile {
                path: "big.rs".into(),
                contents: three_mb,
            }],
            diff: String::new(),
            target: None,
        };
        reviewer.review(&ctx).await.unwrap();
        let prompt = captured.lock().unwrap().clone().unwrap();
        assert!(prompt.contains("## File: big.rs"));
        assert!(
            !prompt.contains("## Skipped (budget exhausted)"),
            "no skip footer expected when cap fits the content"
        );
    }

    /// Default `prompt_budget` from `CodeReviewer::new` matches
    /// `DEFAULT_PROMPT_BUDGET` (the historical hard-coded value).
    #[test]
    fn default_prompt_budget_matches_historical_constant() {
        let (client, _) = stub_with_capture("VERDICT: Pass\n");
        let reviewer = CodeReviewer::new(client, "irrelevant".into());
        assert_eq!(reviewer.prompt_budget(), DEFAULT_PROMPT_BUDGET);
        assert_eq!(reviewer.prompt_budget(), 2_000_000);
    }

    #[test]
    fn build_cross_change_preamble_single_change_is_empty() {
        let briefs = vec![ChangeBrief {
            name: "only-one".into(),
            proposal: "## Why\nfor reasons\n".into(),
            design: None,
            tasks: String::new(),
        }];
        assert_eq!(
            build_cross_change_preamble("only-one", &briefs),
            "",
            "single-change pass produces empty preamble"
        );
    }

    #[test]
    fn build_cross_change_preamble_lists_other_changes() {
        let briefs = vec![
            ChangeBrief {
                name: "a".into(),
                proposal: "## Why\nfix the auth bug\n".into(),
                design: None,
                tasks: String::new(),
            },
            ChangeBrief {
                name: "b".into(),
                proposal: "## Why\nadd metrics emission\n".into(),
                design: None,
                tasks: String::new(),
            },
            ChangeBrief {
                name: "c".into(),
                proposal: "## Why\nrefactor dispatcher\n".into(),
                design: None,
                tasks: String::new(),
            },
        ];
        let p = build_cross_change_preamble("b", &briefs);
        // Must reference the change being reviewed, mention the count,
        // and name the OTHER changes — never itself.
        assert!(p.contains("This PR contains 3 changes"));
        assert!(p.contains("`b`"));
        assert!(p.contains("- a: fix the auth bug"));
        assert!(p.contains("- c: refactor dispatcher"));
        // Must NOT include the reviewed change in the "others" list.
        assert!(!p.contains("- b: add metrics emission"));
        // Must end with the verdict-scope reminder.
        assert!(p.contains("Your verdict applies ONLY to `b`"));
    }

    #[test]
    fn build_cross_change_preamble_truncates_long_why_to_200_chars() {
        // Use a sentinel char that does NOT appear anywhere in the
        // surrounding preamble template, so we can count its occurrences
        // and isolate the truncation behavior.
        let long_why = "Z".repeat(500);
        let briefs = vec![
            ChangeBrief {
                name: "self".into(),
                proposal: "## Why\nshort\n".into(),
                design: None,
                tasks: String::new(),
            },
            ChangeBrief {
                name: "other".into(),
                proposal: format!("## Why\n{long_why}\n"),
                design: None,
                tasks: String::new(),
            },
        ];
        let p = build_cross_change_preamble("self", &briefs);
        // The line is `- other: <truncated to 200 chars>\n`; we expect
        // exactly 200 Z's, not 500.
        let z_count = p.matches('Z').count();
        assert_eq!(z_count, 200);
    }

    #[tokio::test]
    async fn review_per_change_invokes_llm_once_per_change() {
        // We need to track each call. Use the StubClient pattern but
        // record every prompt observed (not just the last).
        use std::sync::Mutex;
        struct CountingClient {
            prompts: Arc<Mutex<Vec<String>>>,
        }
        #[async_trait]
        impl LlmClient for CountingClient {
            async fn complete(&self, prompt: &str) -> anyhow::Result<String> {
                self.prompts.lock().unwrap().push(prompt.to_string());
                Ok("VERDICT: Pass\n".to_string())
            }
        }
        let prompts: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let client = Box::new(CountingClient { prompts: prompts.clone() });
        let template = "PREAMBLE<<{{cross_change_preamble}}>>FILES<<{{changed_files}}>>".to_string();
        let reviewer = CodeReviewer::new(client, template)
            .with_mode(crate::config::ReviewerMode::PerChange);

        let briefs = vec![
            ChangeBrief {
                name: "a".into(),
                proposal: "## Why\nfor a reasons\n".into(),
                design: None,
                tasks: String::new(),
            },
            ChangeBrief {
                name: "b".into(),
                proposal: "## Why\nfor b reasons\n".into(),
                design: None,
                tasks: String::new(),
            },
            ChangeBrief {
                name: "c".into(),
                proposal: "## Why\nfor c reasons\n".into(),
                design: None,
                tasks: String::new(),
            },
        ];
        let contexts: Vec<PerChangeContext> = briefs
            .iter()
            .map(|b| PerChangeContext {
                change_slug: b.name.clone(),
                context: ReviewContext {
                    archived_changes: vec![b.clone()],
                    changed_files: vec![ChangedFile {
                        path: format!("{}.rs", b.name),
                        contents: format!("body of {}", b.name),
                    }],
                    diff: format!("diff of {}", b.name),
                    target: None,
                },
                cross_change_preamble: build_cross_change_preamble(&b.name, &briefs),
            })
            .collect();

        let results = reviewer.review_per_change(&contexts).await.unwrap();
        assert_eq!(results.len(), 3);
        let captured = prompts.lock().unwrap();
        assert_eq!(captured.len(), 3, "one LLM call per change");
        // Each prompt must contain ONLY its own file's body and a
        // preamble naming the OTHER two changes.
        for (i, slug) in ["a", "b", "c"].iter().enumerate() {
            let p = &captured[i];
            assert!(p.contains(&format!("body of {slug}")), "prompt {i}: own body");
            for other in ["a", "b", "c"].iter() {
                if other != slug {
                    assert!(
                        p.contains(&format!("- {other}: for {other} reasons")),
                        "prompt {i}: preamble must name other slug {other}"
                    );
                }
            }
            // Must NOT contain the verdict-scope line for any OTHER change.
            assert!(
                p.contains(&format!("`{slug}`")),
                "prompt {i}: self-reference in preamble"
            );
        }
        for r in &results {
            assert_eq!(r.report.verdict, ReviewVerdict::Pass);
        }
    }

    /// Per-change budget enforcement is per-call: a huge file in change
    /// A produces a skip footer in A's section but does NOT affect B's
    /// or C's reviews.
    #[tokio::test]
    async fn review_per_change_budgets_are_independent() {
        use std::sync::Mutex;
        struct EchoClient {
            prompts: Arc<Mutex<Vec<String>>>,
        }
        #[async_trait]
        impl LlmClient for EchoClient {
            async fn complete(&self, prompt: &str) -> anyhow::Result<String> {
                self.prompts.lock().unwrap().push(prompt.to_string());
                Ok("VERDICT: Pass\n".to_string())
            }
        }
        let prompts: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let client = Box::new(EchoClient { prompts: prompts.clone() });
        let reviewer = CodeReviewer::new(client, "{{changed_files}}".to_string())
            .with_prompt_budget(1_000_000)
            .with_mode(crate::config::ReviewerMode::PerChange);
        // Change A: a huge file (way over the 1MB cap).
        let huge = "z".repeat(2_000_000);
        let a_ctx = PerChangeContext {
            change_slug: "a".into(),
            context: ReviewContext {
                archived_changes: Vec::new(),
                changed_files: vec![ChangedFile {
                    path: "huge.rs".into(),
                    contents: huge,
                }],
                diff: String::new(),
                target: None,
            },
            cross_change_preamble: String::new(),
        };
        // Change B: a tiny file (well under cap).
        let b_ctx = PerChangeContext {
            change_slug: "b".into(),
            context: ReviewContext {
                archived_changes: Vec::new(),
                changed_files: vec![ChangedFile {
                    path: "tiny.rs".into(),
                    contents: "fn ok() {}".into(),
                }],
                diff: String::new(),
                target: None,
            },
            cross_change_preamble: String::new(),
        };
        let _ = reviewer
            .review_per_change(&[a_ctx, b_ctx])
            .await
            .unwrap();
        let captured = prompts.lock().unwrap();
        assert_eq!(captured.len(), 2);
        // A's prompt: skipped footer must fire.
        assert!(
            captured[0].contains("## Skipped (budget exhausted): huge.rs"),
            "change A's huge file must trigger its own skip footer"
        );
        // B's prompt: must contain the tiny file in full, and NO skip footer.
        assert!(captured[1].contains("fn ok() {}"));
        assert!(
            !captured[1].contains("## Skipped (budget exhausted)"),
            "change B's review must NOT be affected by change A's truncation"
        );
    }

    /// Task 5.3: `review_pr_at_state_with` against a stub LLM returning
    /// a canned `Pass` verdict produces `ReviewResult { verdict: Approve, ... }`.
    #[tokio::test]
    async fn review_pr_at_state_approves_on_pass_verdict() {
        let (client, _) = stub_with_capture("VERDICT: Pass\n\nAll good.\n");
        let reviewer = CodeReviewer::new(client, "{{diff}}".to_string());
        let ctx = ReviewContext {
            archived_changes: Vec::new(),
            changed_files: Vec::new(),
            diff: "some diff".to_string(),
            target: None,
        };
        let result = review_pr_at_state_with(&reviewer, &ctx)
            .await
            .expect("review succeeds");
        assert_eq!(result.verdict, Verdict::Approve);
        assert!(result.markdown.contains("All good."));
    }

    /// Task 5.3 cont'd: `Concerns` verdict ALSO maps to `Approve` on the
    /// operator-facing surface.
    #[tokio::test]
    async fn review_pr_at_state_approves_on_concerns_verdict() {
        let (client, _) = stub_with_capture("VERDICT: Concerns\n\nminor nits.\n");
        let reviewer = CodeReviewer::new(client, "{{diff}}".to_string());
        let ctx = ReviewContext {
            archived_changes: Vec::new(),
            changed_files: Vec::new(),
            diff: "some diff".to_string(),
            target: None,
        };
        let result = review_pr_at_state_with(&reviewer, &ctx)
            .await
            .expect("review succeeds");
        assert_eq!(result.verdict, Verdict::Approve);
    }

    /// Task 5.4: `Block` verdict surfaces as `Block` AND any concerns
    /// from the trailing `revision-requests` block are preserved.
    #[tokio::test]
    async fn review_pr_at_state_blocks_on_block_verdict() {
        let raw = "VERDICT: Block\n\nSerious issue.\n\n```revision-requests\n- summary: \"fix the broken thing\"\n  should_request_revision: true\n```\n";
        let (client, _) = stub_with_capture(raw);
        let reviewer = CodeReviewer::new(client, "{{diff}}".to_string());
        let ctx = ReviewContext {
            archived_changes: Vec::new(),
            changed_files: Vec::new(),
            diff: "some diff".to_string(),
            target: None,
        };
        let result = review_pr_at_state_with(&reviewer, &ctx)
            .await
            .expect("review succeeds");
        assert_eq!(result.verdict, Verdict::Block);
        assert_eq!(result.per_concern.len(), 1);
        assert!(result.per_concern[0].should_request_revision);
        assert_eq!(result.per_concern[0].summary, "fix the broken thing");
    }

    /// Task 5.5: the extracted entry point's output (markdown body) is
    /// byte-identical to what `CodeReviewer::review`'s `ReviewReport`
    /// would have produced for the same inputs. Confirms the
    /// extraction is refactor-only.
    #[tokio::test]
    async fn review_pr_at_state_byte_identical_to_review_report() {
        let raw = "VERDICT: Pass\n\nNothing of note.\n";
        let (client_a, _) = stub_with_capture(raw);
        let reviewer_a = CodeReviewer::new(client_a, "{{diff}}".to_string());
        let ctx_a = ReviewContext {
            archived_changes: Vec::new(),
            changed_files: Vec::new(),
            diff: "abc".to_string(),
            target: None,
        };
        let report = reviewer_a.review(&ctx_a).await.unwrap();

        let (client_b, _) = stub_with_capture(raw);
        let reviewer_b = CodeReviewer::new(client_b, "{{diff}}".to_string());
        let ctx_b = ReviewContext {
            archived_changes: Vec::new(),
            changed_files: Vec::new(),
            diff: "abc".to_string(),
            target: None,
        };
        let result = review_pr_at_state_with(&reviewer_b, &ctx_b)
            .await
            .unwrap();
        assert_eq!(result.markdown, report.markdown);
        assert_eq!(result.concerns.len(), report.concerns.len());
        assert_eq!(Verdict::from(report.verdict), result.verdict);
    }

    /// a53 task 3.1: `review_pr_at_state_with` over a synthetic 3-change
    /// `ReviewContext` in per_change mode invokes the reviewer once per
    /// change (3 calls) AND returns a `ReviewResult` carrying 3
    /// `per_change_sections`, one per change, in input order. This is the
    /// regression the change pins: the operator-trigger entry point now
    /// honors `reviewer.mode == per_change` instead of always bundling.
    #[tokio::test]
    async fn review_pr_at_state_per_change_dispatches_once_per_change() {
        use std::sync::Mutex;
        struct CountingClient {
            prompts: Arc<Mutex<Vec<String>>>,
        }
        #[async_trait]
        impl LlmClient for CountingClient {
            async fn complete(&self, prompt: &str) -> anyhow::Result<String> {
                self.prompts.lock().unwrap().push(prompt.to_string());
                Ok("VERDICT: Pass\n\nlooks fine\n".to_string())
            }
        }
        let prompts: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let client = Box::new(CountingClient { prompts: prompts.clone() });
        let reviewer = CodeReviewer::new(
            client,
            "{{cross_change_preamble}}{{changed_files}}{{diff}}".to_string(),
        )
        .with_mode(crate::config::ReviewerMode::PerChange);

        let brief = |name: &str| ChangeBrief {
            name: name.into(),
            proposal: format!("## Why\nreasons for {name}\n"),
            design: None,
            tasks: String::new(),
        };
        let ctx = ReviewContext {
            archived_changes: vec![brief("alpha"), brief("beta"), brief("gamma")],
            changed_files: vec![ChangedFile {
                path: "src/x.rs".into(),
                contents: "fn x() {}".into(),
            }],
            diff: "the union diff".into(),
            target: None,
        };

        let result = review_pr_at_state_with(&reviewer, &ctx)
            .await
            .expect("per-change review succeeds");
        assert_eq!(prompts.lock().unwrap().len(), 3, "one LLM call per change");
        assert_eq!(result.per_change_sections.len(), 3);
        let slugs: Vec<&str> = result
            .per_change_sections
            .iter()
            .map(|s| s.change_slug.as_str())
            .collect();
        assert_eq!(slugs, ["alpha", "beta", "gamma"]);
        for s in &result.per_change_sections {
            assert!(
                s.markdown.starts_with("VERDICT: "),
                "each section body carries its own verdict line"
            );
        }
    }

    /// a53 task 3.2: in bundled mode `review_pr_at_state_with` invokes the
    /// reviewer exactly once, returns empty `per_change_sections`, AND its
    /// markdown is byte-identical to a direct `CodeReviewer::review` of the
    /// same context — no behavior change for the default path.
    #[tokio::test]
    async fn review_pr_at_state_bundled_single_call_empty_sections() {
        use std::sync::Mutex;
        struct CountingClient {
            prompts: Arc<Mutex<Vec<String>>>,
            response: String,
        }
        #[async_trait]
        impl LlmClient for CountingClient {
            async fn complete(&self, prompt: &str) -> anyhow::Result<String> {
                self.prompts.lock().unwrap().push(prompt.to_string());
                Ok(self.response.clone())
            }
        }
        let raw = "VERDICT: Concerns\n\nminor nit.\n";
        let make_ctx = || ReviewContext {
            archived_changes: vec![
                ChangeBrief {
                    name: "alpha".into(),
                    proposal: "## Why\na\n".into(),
                    design: None,
                    tasks: String::new(),
                },
                ChangeBrief {
                    name: "beta".into(),
                    proposal: "## Why\nb\n".into(),
                    design: None,
                    tasks: String::new(),
                },
            ],
            changed_files: vec![ChangedFile {
                path: "src/x.rs".into(),
                contents: "fn x() {}".into(),
            }],
            diff: "the diff".into(),
            target: None,
        };

        // Reference: a direct bundled review of the same context.
        let (ref_client, _) = stub_with_capture(raw);
        let ref_reviewer = CodeReviewer::new(ref_client, "{{changed_files}}{{diff}}".to_string());
        let ref_report = ref_reviewer.review(&make_ctx()).await.unwrap();

        // System under test: the bundled-mode entry point.
        let prompts: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let client = Box::new(CountingClient {
            prompts: prompts.clone(),
            response: raw.to_string(),
        });
        let reviewer = CodeReviewer::new(client, "{{changed_files}}{{diff}}".to_string())
            .with_mode(crate::config::ReviewerMode::Bundled);
        let result = review_pr_at_state_with(&reviewer, &make_ctx()).await.unwrap();

        assert_eq!(
            prompts.lock().unwrap().len(),
            1,
            "bundled mode: exactly one LLM call regardless of change count"
        );
        assert!(
            result.per_change_sections.is_empty(),
            "bundled mode leaves per_change_sections empty"
        );
        assert_eq!(
            result.markdown, ref_report.markdown,
            "bundled output byte-identical to a direct review"
        );
        assert_eq!(Verdict::from(ref_report.verdict), result.verdict);
    }

    /// a015 task 2.1: `per_change` mode with an empty `archived_changes`
    /// context (the split yields zero sub-contexts) but a non-empty
    /// diff/changed_files falls back to a single bundled review. Exactly
    /// one reviewer invocation occurs AND the verdict is the one the
    /// stubbed bundled review returns — NOT a defaulted `Pass`/`Approve`
    /// synthesized from zero reviews. The stub returns `Block` precisely
    /// because `Block` is the only verdict that does not map to `Approve`:
    /// if the pre-a015 bug were present (empty synthesis → `Pass` →
    /// `Approve`), this assertion would fail.
    #[tokio::test]
    async fn per_change_empty_split_falls_back_to_bundled_with_real_verdict() {
        use std::sync::Mutex;
        struct CountingClient {
            prompts: Arc<Mutex<Vec<String>>>,
        }
        #[async_trait]
        impl LlmClient for CountingClient {
            async fn complete(&self, prompt: &str) -> anyhow::Result<String> {
                self.prompts.lock().unwrap().push(prompt.to_string());
                Ok("VERDICT: Block\n\nbundled review found a real problem\n".to_string())
            }
        }
        let prompts: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let client = Box::new(CountingClient { prompts: prompts.clone() });
        let reviewer = CodeReviewer::new(client, "{{changed_files}}{{diff}}".to_string())
            .with_mode(crate::config::ReviewerMode::PerChange);

        // Empty archived_changes → split yields zero sub-contexts, but the
        // PR still has a real diff and changed files to review.
        let ctx = ReviewContext {
            archived_changes: Vec::new(),
            changed_files: vec![ChangedFile {
                path: "src/x.rs".into(),
                contents: "fn x() {}".into(),
            }],
            diff: "the union diff".into(),
            target: None,
        };

        let result = review_pr_at_state_with(&reviewer, &ctx)
            .await
            .expect("fallback bundled review succeeds");

        assert_eq!(
            prompts.lock().unwrap().len(),
            1,
            "empty split falls back to exactly one bundled reviewer invocation"
        );
        assert_eq!(
            result.verdict,
            Verdict::Block,
            "verdict comes from the bundled review, not a defaulted Pass/Approve"
        );
        assert!(
            result.per_change_sections.is_empty(),
            "the fallback is a bundled review — no per-change sections"
        );
        assert!(result.markdown.contains("bundled review found a real problem"));
    }

    /// a015 task 2.2: the fallback bundled review is handed the context's
    /// diff and changed files (asserting on what the stub reviewer
    /// received, not on any log/message wording). Proves the reviewer
    /// builds its prompt over the real context rather than skipping the
    /// call.
    #[tokio::test]
    async fn per_change_empty_split_fallback_passes_diff_and_files() {
        let (client, captured) = stub_with_capture("VERDICT: Concerns\n\nnit\n");
        let reviewer = CodeReviewer::new(client, "{{changed_files}}{{diff}}".to_string())
            .with_mode(crate::config::ReviewerMode::PerChange);

        let ctx = ReviewContext {
            archived_changes: Vec::new(),
            changed_files: vec![ChangedFile {
                path: "src/touched.rs".into(),
                contents: "FILE_BODY_SENTINEL_a015".into(),
            }],
            diff: "DIFF_SENTINEL_a015".into(),
            target: None,
        };

        let _ = review_pr_at_state_with(&reviewer, &ctx)
            .await
            .expect("fallback bundled review succeeds");

        let prompt = captured
            .lock()
            .unwrap()
            .clone()
            .expect("the reviewer built and submitted a prompt");
        assert!(
            prompt.contains("DIFF_SENTINEL_a015"),
            "the fallback review receives the context's diff"
        );
        assert!(
            prompt.contains("FILE_BODY_SENTINEL_a015"),
            "the fallback review receives the context's changed files"
        );
    }

    /// a015 task 2.3 (regression): `per_change` mode with a populated
    /// `archived_changes` (≥1 change) still dispatches one review per
    /// change and synthesizes the results — no bundled fallback fires.
    #[tokio::test]
    async fn per_change_populated_split_still_dispatches_per_change() {
        use std::sync::Mutex;
        struct CountingClient {
            prompts: Arc<Mutex<Vec<String>>>,
        }
        #[async_trait]
        impl LlmClient for CountingClient {
            async fn complete(&self, prompt: &str) -> anyhow::Result<String> {
                self.prompts.lock().unwrap().push(prompt.to_string());
                Ok("VERDICT: Pass\n\nlooks fine\n".to_string())
            }
        }
        let prompts: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let client = Box::new(CountingClient { prompts: prompts.clone() });
        let reviewer = CodeReviewer::new(
            client,
            "{{cross_change_preamble}}{{changed_files}}{{diff}}".to_string(),
        )
        .with_mode(crate::config::ReviewerMode::PerChange);

        let brief = |name: &str| ChangeBrief {
            name: name.into(),
            proposal: format!("## Why\nreasons for {name}\n"),
            design: None,
            tasks: String::new(),
        };
        let ctx = ReviewContext {
            archived_changes: vec![brief("alpha"), brief("beta")],
            changed_files: vec![ChangedFile {
                path: "src/x.rs".into(),
                contents: "fn x() {}".into(),
            }],
            diff: "the union diff".into(),
            target: None,
        };

        let result = review_pr_at_state_with(&reviewer, &ctx)
            .await
            .expect("per-change review succeeds");

        assert_eq!(
            prompts.lock().unwrap().len(),
            2,
            "one reviewer invocation per change — no bundled fallback"
        );
        let slugs: Vec<&str> = result
            .per_change_sections
            .iter()
            .map(|s| s.change_slug.as_str())
            .collect();
        assert_eq!(
            slugs,
            ["alpha", "beta"],
            "results are synthesized per change, in input order"
        );
    }

    /// a015 task 1.2: the empty-input guard on `synthesize_per_change_report`
    /// makes the "never a defaulted Pass" invariant explicit. Called with
    /// an empty vec it returns a non-`Pass` (here `Block`) verdict so a
    /// synthesis from zero reviews can never become a silent approval.
    #[test]
    fn synthesize_per_change_report_empty_input_is_not_pass() {
        let report = synthesize_per_change_report(Vec::new());
        assert_ne!(
            report.verdict,
            ReviewVerdict::Pass,
            "an empty per-change synthesis must never default to Pass"
        );
        assert_ne!(
            Verdict::from(report.verdict),
            Verdict::Approve,
            "an empty per-change synthesis must never map to Approve"
        );
        assert!(report.per_change_sections.is_empty());
        assert!(report.concerns.is_empty());
    }

    /// Behavior test (a48): the shipped default template must reference
    /// all three substitution placeholders, because the production render
    /// path (`review_with_preamble`) fills them. Rendering the real
    /// default with a distinct sentinel per placeholder and asserting each
    /// sentinel survives proves the references exist — without pinning any
    /// of the template's hand-authored instruction prose (per the
    /// project-documentation requirement "Tests assert behavior or
    /// derivation, never message wording").
    #[test]
    fn default_template_references_all_placeholders() {
        let rendered = DEFAULT_TEMPLATE
            .replace("{{change_context}}", "SENTINEL_CHANGE_CONTEXT_a48")
            .replace("{{changed_files}}", "SENTINEL_CHANGED_FILES_a48")
            .replace("{{diff}}", "SENTINEL_DIFF_a48");
        assert!(
            rendered.contains("SENTINEL_CHANGE_CONTEXT_a48"),
            "default template must reference the {{change_context}} placeholder"
        );
        assert!(
            rendered.contains("SENTINEL_CHANGED_FILES_a48"),
            "default template must reference the {{changed_files}} placeholder"
        );
        assert!(
            rendered.contains("SENTINEL_DIFF_a48"),
            "default template must reference the {{diff}} placeholder"
        );
    }

    /// a002 regression (task 3.4): a `ReviewContext` whose changed files
    /// contain the literal `{{diff}}` AND `{{changed_files}}` tokens — the
    /// self-hosting case where the change under review edits the reviewer's
    /// own spec/code/docs — renders a prompt that does NOT re-expand those
    /// literals. Under the old chained `.replace`, the final
    /// `.replace("{{diff}}", …)` stamped the whole diff into every literal
    /// `{{diff}}` carried in the changed files, exploding the prompt.
    #[tokio::test]
    async fn changed_file_placeholder_literals_are_not_re_expanded() {
        let (client, captured) = stub_with_capture("VERDICT: Pass\n");
        // A realistic template wraps each section in delimiters.
        let template =
            "CTX<<<{{change_context}}>>>\nFILES<<<{{changed_files}}>>>\nDIFF<<<{{diff}}>>>"
                .to_string();
        let reviewer = CodeReviewer::new(client, template.clone());

        // The changed file's contents carry MANY literal placeholder tokens
        // (as the reviewer's own spec docs do). The diff itself is large so
        // that re-expansion would be conspicuous.
        let file_contents = "documents {{diff}} and {{changed_files}} tokens\n".repeat(50);
        let diff = "D".repeat(10_000);
        let ctx = ReviewContext {
            archived_changes: Vec::new(),
            changed_files: vec![ChangedFile {
                path: "openspec/specs/code-reviewer/spec.md".into(),
                contents: file_contents.clone(),
            }],
            diff: diff.clone(),
            target: None,
        };
        reviewer.review(&ctx).await.unwrap();
        let prompt = captured.lock().unwrap().clone().unwrap();

        // The literal tokens survive verbatim in the changed-files section.
        assert!(
            prompt.contains("documents {{diff}} and {{changed_files}} tokens"),
            "literal placeholder tokens must survive verbatim in the changed-files section"
        );
        // The big diff is inserted exactly once (at the template's own
        // `{{diff}}`), NOT once per literal carried in the file.
        assert_eq!(
            prompt.matches(&diff).count(),
            1,
            "the diff must be inserted exactly once, not re-stamped into every literal"
        );

        // Size bound: the rendered prompt cannot exceed the sum of the
        // section values plus the template scaffolding. (`render_sections`
        // builds the changed-files section with `## File:` headers, so we
        // bound by file_contents + a small per-file header allowance rather
        // than by the bare contents.)
        let rendered = render_sections(&ctx, reviewer.prompt_budget());
        let bound = rendered.change_context.len()
            + rendered.changed_files.len()
            + rendered.diff_or_explanation.len()
            + template.len();
        assert!(
            prompt.len() <= bound,
            "prompt size {} must be bounded by section sizes + template = {bound} \
             (no multiplicative blowup)",
            prompt.len()
        );
    }

    // =================================================================
    // a58: agentic reviewer transport
    // =================================================================

    use serde_json::json;
    use std::collections::VecDeque;

    fn brief(name: &str) -> ChangeBrief {
        ChangeBrief {
            name: name.into(),
            proposal: "## Why\nbecause reasons".into(),
            design: None,
            tasks: "- [x] do the thing".into(),
        }
    }

    fn valid_review_payload(verdict: &str) -> serde_json::Value {
        json!({ "verdict": verdict, "summary": "looks ok", "concerns": [] })
    }

    /// Test session runner: records the slugs + prompts it saw AND returns
    /// canned submissions (front-of-queue), bypassing any CLI spawn.
    struct CannedRunner {
        submissions: Mutex<VecDeque<Option<serde_json::Value>>>,
        slugs: Mutex<Vec<String>>,
        prompts: Mutex<Vec<String>>,
        diffs: Mutex<Vec<String>>,
    }
    impl CannedRunner {
        fn new(subs: Vec<Option<serde_json::Value>>) -> Self {
            Self {
                submissions: Mutex::new(subs.into_iter().collect()),
                slugs: Mutex::new(Vec::new()),
                prompts: Mutex::new(Vec::new()),
                diffs: Mutex::new(Vec::new()),
            }
        }
        fn session_count(&self) -> usize {
            self.slugs.lock().unwrap().len()
        }
    }
    #[async_trait]
    impl ReviewSessionRunner for CannedRunner {
        async fn run_session(&self, slug: &str, prompt: &str, diff: &str) -> Result<Option<Value>> {
            self.slugs.lock().unwrap().push(slug.to_string());
            self.prompts.lock().unwrap().push(prompt.to_string());
            self.diffs.lock().unwrap().push(diff.to_string());
            let next = self.submissions.lock().unwrap().pop_front();
            Ok(next.unwrap_or(None))
        }
    }

    /// The `oneshot` transport's prompt + parsed output are byte-identical
    /// to the pre-change one-shot path (the agentic branch is never taken).
    /// `CodeReviewer::new` is the test-only constructor and keeps `oneshot`
    /// so this surface exercises the HTTP path directly; the operator-facing
    /// `reviewer.kind` config default is `agentic` since a64 (see
    /// `config::ReviewerKind` AND `startup_reviewer_kind_decision`).
    #[tokio::test]
    async fn oneshot_kind_is_byte_identical() {
        let (client, captured) = stub_with_capture("VERDICT: Pass\n\nthe review body");
        let reviewer = CodeReviewer::new(client, "{{diff}}".to_string());
        assert_eq!(
            reviewer.kind(),
            ReviewerKind::Oneshot,
            "the test-only `new` constructor keeps oneshot"
        );
        let ctx = ctx_with_diff("DIFFTEXT");
        let result = review_pr_at_state_with(&reviewer, &ctx).await.unwrap();
        // The one-shot prompt is the unchanged render: the bare diff for a
        // `{{diff}}`-only template — no agentic briefs/file-list framing.
        let prompt = captured.lock().unwrap().clone().unwrap();
        assert_eq!(prompt, "DIFFTEXT");
        assert_eq!(result.verdict, Verdict::Approve);
        assert_eq!(result.markdown, "the review body");
    }

    // =================================================================
    // a64: startup CLI-availability fallback (tasks 3.1–3.4)
    // =================================================================

    /// An absolute path to a file that is guaranteed to exist on the host
    /// (the running test binary). `reviewer_binary_on_path` treats a
    /// path-qualified command as "available" when the file exists, giving the
    /// "CLI present" branch a deterministic input that does not depend on
    /// what bare-name binaries happen to be on the CI `$PATH`.
    fn present_cli() -> String {
        std::env::current_exe()
            .expect("current_exe resolves in tests")
            .to_string_lossy()
            .into_owned()
    }

    /// A bare command name guaranteed NOT to be on any sane `$PATH`, so
    /// `reviewer_binary_on_path` reports it missing.
    const MISSING_CLI: &str = "autocoder-a64-definitely-not-installed-cli";

    /// 3.1: unset `reviewer.kind` resolves to agentic AND, with an available
    /// reviewer CLI, the startup resolver keeps the reviewer agentic with no
    /// fallback WARN.
    #[test]
    fn unset_kind_with_available_cli_stays_agentic() {
        // Unset kind defaults to agentic (the `new` test constructor is
        // oneshot, so model the config default explicitly).
        assert_eq!(ReviewerKind::default(), ReviewerKind::Agentic);

        let (client, _captured) = stub_with_capture("");
        let reviewer = CodeReviewer::new(client, "{{diff}}".to_string())
            .with_kind(ReviewerKind::Agentic)
            .with_command(present_cli());
        let (effective, warn) = resolve_startup_reviewer_kind(&reviewer);
        assert_eq!(effective, ReviewerKind::Agentic, "available CLI → agentic");
        assert!(warn.is_none(), "no WARN when the CLI is available: {warn:?}");
    }

    /// 3.2 / 3.3: an effective-agentic reviewer (defaulted OR explicit) whose
    /// CLI is unavailable degrades to `oneshot` for the boot AND emits exactly
    /// one WARN naming the CLI + the remedy. Review is NOT disabled — the
    /// effective kind is `oneshot`, not "off".
    #[test]
    fn agentic_with_unavailable_cli_falls_back_to_oneshot_with_warn() {
        let (client, _captured) = stub_with_capture("");
        let reviewer = CodeReviewer::new(client, "{{diff}}".to_string())
            .with_kind(ReviewerKind::Agentic)
            .with_command(MISSING_CLI.to_string());
        let (effective, warn) = resolve_startup_reviewer_kind(&reviewer);
        assert_eq!(
            effective,
            ReviewerKind::Oneshot,
            "missing CLI → oneshot fallback (review continues, not disabled)"
        );
        let warn = warn.expect("missing CLI must produce a fallback WARN");
        assert!(
            warn.contains(MISSING_CLI),
            "WARN must name the missing CLI: {warn}"
        );
        assert!(
            warn.contains("oneshot"),
            "WARN must name the `reviewer.kind: oneshot` remedy: {warn}"
        );
    }

    /// 3.4: an explicit `oneshot` reviewer is honored with no probe, no
    /// agentic session, AND no fallback WARN — even when the CLI is missing
    /// (the operator opted out deliberately).
    #[test]
    fn explicit_oneshot_is_honored_without_warn() {
        let (client, _captured) = stub_with_capture("");
        let reviewer = CodeReviewer::new(client, "{{diff}}".to_string())
            .with_kind(ReviewerKind::Oneshot)
            .with_command(MISSING_CLI.to_string());
        let (effective, warn) = resolve_startup_reviewer_kind(&reviewer);
        assert_eq!(effective, ReviewerKind::Oneshot);
        assert!(warn.is_none(), "explicit oneshot never warns: {warn:?}");
    }

    /// The pure decision function covers all four arms independently of any
    /// host probe (tasks 3.1–3.4 condensed): oneshot is always honored
    /// warning-free; agentic + available stays agentic; agentic + unavailable
    /// degrades to oneshot with a CLI-naming, remedy-naming WARN.
    #[test]
    fn startup_kind_decision_truth_table() {
        // Oneshot configured: honored, never warns, regardless of availability.
        for available in [true, false] {
            assert_eq!(
                startup_reviewer_kind_decision(ReviewerKind::Oneshot, "claude", available),
                (ReviewerKind::Oneshot, None)
            );
        }
        // Agentic + available CLI: agentic, no warn.
        assert_eq!(
            startup_reviewer_kind_decision(ReviewerKind::Agentic, "claude", true),
            (ReviewerKind::Agentic, None)
        );
        // Agentic + unavailable CLI: oneshot + WARN naming the CLI and remedy.
        let (kind, warn) =
            startup_reviewer_kind_decision(ReviewerKind::Agentic, "qwen-cli", false);
        assert_eq!(kind, ReviewerKind::Oneshot);
        let warn = warn.expect("unavailable agentic CLI warns");
        assert!(warn.contains("qwen-cli"), "names the CLI: {warn}");
        assert!(warn.contains("oneshot"), "names the remedy: {warn}");
    }

    /// `reviewer_binary_on_path` finds a real file via an absolute path AND
    /// reports a bare name absent from `$PATH` as missing — the primitive the
    /// startup resolver's binary check rests on.
    #[test]
    fn binary_on_path_detects_present_and_missing() {
        assert!(
            reviewer_binary_on_path(&present_cli()),
            "an absolute path to an existing file is available"
        );
        assert!(
            !reviewer_binary_on_path(MISSING_CLI),
            "a bare name not on PATH is unavailable"
        );
    }

    // =================================================================
    // a59: on-demand review of a PR, commit, or target
    // =================================================================

    /// `ReviewTargetSpec::parse` recognizes the keyword forms AND falls back
    /// to a free-text description; malformed keyword forms error.
    #[test]
    fn review_target_spec_parses_each_form() {
        let toks = |s: &str| s.split_whitespace().map(String::from).collect::<Vec<_>>();
        assert_eq!(
            ReviewTargetSpec::parse(&toks("pr 42")).unwrap(),
            ReviewTargetSpec::Pr { number: 42 }
        );
        assert_eq!(
            ReviewTargetSpec::parse(&toks("commit abc123")).unwrap(),
            ReviewTargetSpec::Commit {
                sha: "abc123".into()
            }
        );
        assert_eq!(
            ReviewTargetSpec::parse(&toks("files src/a.rs src/b.rs")).unwrap(),
            ReviewTargetSpec::Files {
                paths: vec!["src/a.rs".into(), "src/b.rs".into()]
            }
        );
        // Free text → description.
        assert_eq!(
            ReviewTargetSpec::parse(&toks("the queue blocking logic")).unwrap(),
            ReviewTargetSpec::Description {
                focus: "the queue blocking logic".into()
            }
        );
        // Malformed forms error.
        assert!(ReviewTargetSpec::parse(&toks("pr notanumber")).is_err());
        assert!(ReviewTargetSpec::parse(&toks("pr")).is_err());
        assert!(ReviewTargetSpec::parse(&toks("files")).is_err());
        assert!(ReviewTargetSpec::parse(&[]).is_err());
    }

    /// `chunk_target_files` keeps a bounded list whole AND splits an oversized
    /// one into consecutive bounded groups.
    #[test]
    fn chunk_target_files_bounds_groups() {
        let files: Vec<String> = (0..5).map(|i| format!("f{i}.rs")).collect();
        // Bounded: one chunk.
        let chunks = chunk_target_files(&files, 10);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 5);
        // Oversized: split into ceil(5/2) = 3 chunks of <= 2.
        let chunks = chunk_target_files(&files, 2);
        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|c| c.len() <= 2));
        assert_eq!(chunks.iter().map(|c| c.len()).sum::<usize>(), 5);
    }

    /// 5.2: a `files` target runs a NO-DIFF target review — the rendered
    /// prompt carries the focus + file list in place of a diff, AND the
    /// session receives an empty diff. Asserts on what the runner saw.
    #[tokio::test]
    async fn on_demand_files_target_is_no_diff_review() {
        let (client, _) = stub_with_capture("");
        let reviewer = CodeReviewer::new(client, "t".to_string());
        let surface = ReviewSurface::Target(ReviewTarget::Files {
            paths: vec!["src/queue.rs".into(), "src/lib.rs".into()],
        });
        let runner = CannedRunner::new(vec![Some(valid_review_payload("Approve"))]);
        let outcome = run_on_demand_review_with_runner(&reviewer, &surface, Vec::new(), &runner)
            .await
            .unwrap();
        match outcome {
            OnDemandReviewOutcome::Reviewed(r) => {
                assert_eq!(r.verdict, Verdict::Approve);
                assert_eq!(r.sessions, 1);
            }
            OnDemandReviewOutcome::Discarded { .. } => panic!("expected a reviewed outcome"),
        }
        assert_eq!(runner.session_count(), 1);
        // The session's diff is empty (target review, no diff).
        assert_eq!(runner.diffs.lock().unwrap().as_slice(), [String::new()]);
        // The prompt carries the file list, NOT a unified-diff section.
        let prompt = runner.prompts.lock().unwrap()[0].clone();
        assert!(prompt.contains("# Target files"), "prompt: {prompt}");
        assert!(prompt.contains("src/queue.rs"), "prompt names the file: {prompt}");
        assert!(
            !prompt.contains("# Unified diff"),
            "a target review carries no unified-diff section: {prompt}"
        );
    }

    /// 5.3: a description target reviews agent-located files — the prompt
    /// carries the operator's focus AND instructs the agent to locate the
    /// files via Glob/Grep AND to name what it reviewed.
    #[tokio::test]
    async fn on_demand_description_target_locates_files() {
        let (client, _) = stub_with_capture("");
        let reviewer = CodeReviewer::new(client, "t".to_string());
        let surface = ReviewSurface::Target(ReviewTarget::Description {
            focus: "the [out] gate handling".into(),
        });
        let runner = CannedRunner::new(vec![Some(valid_review_payload("Block"))]);
        let outcome = run_on_demand_review_with_runner(&reviewer, &surface, Vec::new(), &runner)
            .await
            .unwrap();
        match outcome {
            OnDemandReviewOutcome::Reviewed(r) => {
                assert_eq!(r.verdict, Verdict::Block);
                assert_eq!(r.sessions, 1, "a description is one session — the agent self-scopes");
            }
            OnDemandReviewOutcome::Discarded { .. } => panic!("expected a reviewed outcome"),
        }
        let prompt = runner.prompts.lock().unwrap()[0].clone();
        assert!(prompt.contains("the [out] gate handling"), "carries the focus: {prompt}");
        assert!(
            prompt.contains("Glob") && prompt.contains("Grep"),
            "instructs the agent to locate files itself: {prompt}"
        );
        assert!(
            prompt.to_lowercase().contains("name the files"),
            "asks the agent to name the files it reviewed: {prompt}"
        );
    }

    /// 5.1: a `pr`/`commit` diff surface runs the reviewer over the resolved
    /// diff (a single bounded session) AND the verdict is reported.
    #[tokio::test]
    async fn on_demand_diff_surface_runs_reviewer_and_reports_verdict() {
        let (client, _) = stub_with_capture("");
        let reviewer = CodeReviewer::new(client, "t".to_string());
        let surface = ReviewSurface::Diff {
            diff: "DIFF_BODY_a59".into(),
            changed_files: vec!["src/x.rs".into()],
        };
        let runner = CannedRunner::new(vec![Some(valid_review_payload("Approve"))]);
        let outcome = run_on_demand_review_with_runner(&reviewer, &surface, Vec::new(), &runner)
            .await
            .unwrap();
        match outcome {
            OnDemandReviewOutcome::Reviewed(r) => {
                assert_eq!(r.verdict, Verdict::Approve);
                assert_eq!(r.sessions, 1);
            }
            OnDemandReviewOutcome::Discarded { .. } => panic!("expected a reviewed outcome"),
        }
        // The diff reaches the session (written to the artifact the agent reads).
        assert_eq!(
            runner.diffs.lock().unwrap().as_slice(),
            ["DIFF_BODY_a59".to_string()]
        );
        let prompt = runner.prompts.lock().unwrap()[0].clone();
        assert!(prompt.contains("# Unified diff"), "diff surface renders a diff section: {prompt}");
    }

    /// 5.4: a target spanning more files than one bounded session is split
    /// into multiple sessions AND the findings aggregate into ONE report.
    #[tokio::test]
    async fn on_demand_large_target_chunks_and_aggregates() {
        let (client, _) = stub_with_capture("");
        let reviewer = CodeReviewer::new(client, "t".to_string());
        // 45 files at a 20-per-session ceiling → 3 chunked sessions.
        let paths: Vec<String> = (0..45).map(|i| format!("src/f{i}.rs")).collect();
        let surface = ReviewSurface::Target(ReviewTarget::Files { paths });
        // One submission per chunk; the middle one Blocks so the aggregate
        // verdict is Block (any chunk blocks → blocks).
        let runner = CannedRunner::new(vec![
            Some(valid_review_payload("Approve")),
            Some(valid_review_payload("Block")),
            Some(valid_review_payload("Approve")),
        ]);
        let outcome = run_on_demand_review_with_runner(&reviewer, &surface, Vec::new(), &runner)
            .await
            .unwrap();
        match outcome {
            OnDemandReviewOutcome::Reviewed(r) => {
                assert_eq!(r.sessions, 3, "45 files / 20-per-session → 3 chunked sessions");
                assert_eq!(r.chunk_labels.len(), 3, "one report aggregating 3 chunks");
                assert_eq!(r.verdict, Verdict::Block, "any chunk Block → aggregate Block");
            }
            OnDemandReviewOutcome::Discarded { .. } => panic!("expected a reviewed outcome"),
        }
        assert_eq!(runner.session_count(), 3, ">1 session ran");
    }

    /// 5.5 (fail-closed): a chunk session that records no valid verdict
    /// discards the WHOLE review — never a defaulted clean pass.
    #[tokio::test]
    async fn on_demand_no_verdict_session_fails_closed() {
        let (client, _) = stub_with_capture("");
        let reviewer = CodeReviewer::new(client, "t".to_string());
        let surface = ReviewSurface::Diff {
            diff: "d".into(),
            changed_files: vec!["src/x.rs".into()],
        };
        // The single session records NO submission.
        let runner = CannedRunner::new(vec![None]);
        let outcome = run_on_demand_review_with_runner(&reviewer, &surface, Vec::new(), &runner)
            .await
            .unwrap();
        match outcome {
            OnDemandReviewOutcome::Discarded { reason } => {
                assert!(
                    reason.contains("no valid submit_review"),
                    "discard reason names the missing submission: {reason}"
                );
            }
            OnDemandReviewOutcome::Reviewed(_) => {
                panic!("a no-verdict session must discard, never report a clean pass")
            }
        }
    }

    /// 5.1 (resolution): a `commit <sha>` target resolves to that commit's
    /// diff + its changed files from the local clone (real git fixture).
    #[test]
    fn resolve_commit_target_produces_diff_and_files() {
        use std::process::Command;
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().to_path_buf();
        let git = |args: &[&str]| {
            let st = Command::new("git").args(args).current_dir(&path).status().unwrap();
            assert!(st.success(), "git {args:?} failed");
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "t@e.com"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(path.join("a.rs"), "fn a() {}\n").unwrap();
        git(&["add", "a.rs"]);
        git(&["commit", "-q", "-m", "first"]);
        std::fs::write(path.join("b.rs"), "fn b() {}\n").unwrap();
        git(&["add", "b.rs"]);
        git(&["commit", "-q", "-m", "second"]);
        let sha = String::from_utf8(
            Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(&path)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();

        let spec = ReviewTargetSpec::Commit { sha };
        let surface = resolve_review_surface(&spec, &path, "main", "origin").unwrap();
        match surface {
            ReviewSurface::Diff { diff, changed_files } => {
                assert!(diff.contains("b.rs"), "commit diff mentions the touched file: {diff}");
                assert!(diff.contains("fn b()"), "commit diff carries the added body: {diff}");
                assert_eq!(changed_files, vec!["b.rs".to_string()]);
            }
            ReviewSurface::Target(_) => panic!("a commit resolves to a Diff surface"),
        }
    }

    /// 5.2 / 5.3 (resolution): `files` AND a description resolve to TARGET
    /// surfaces (no diff). No git is needed — they carry no diff.
    #[test]
    fn resolve_files_and_description_targets_are_diffless() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().to_path_buf();

        let files = resolve_review_surface(
            &ReviewTargetSpec::Files {
                paths: vec!["src/a.rs".into(), "src/b.rs".into()],
            },
            &path,
            "main",
            "origin",
        )
        .unwrap();
        match files {
            ReviewSurface::Target(ReviewTarget::Files { paths }) => {
                assert_eq!(paths, vec!["src/a.rs".to_string(), "src/b.rs".to_string()]);
            }
            other => panic!("files target must resolve to a Files target surface: {other:?}"),
        }

        let desc = resolve_review_surface(
            &ReviewTargetSpec::Description {
                focus: "the queue logic".into(),
            },
            &path,
            "main",
            "origin",
        )
        .unwrap();
        match desc {
            ReviewSurface::Target(ReviewTarget::Description { focus }) => {
                assert_eq!(focus, "the queue logic");
            }
            other => panic!("a description must resolve to a Description target surface: {other:?}"),
        }
    }
}
