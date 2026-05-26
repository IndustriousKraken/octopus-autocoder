//! AI-driven code-quality reviewer. Sends a structured `ReviewContext`
//! (changed-file contents + change-spec context + diff) to a configured LLM
//! and parses the response into a `ReviewReport`. Scope is deliberately
//! code-quality only; spec compliance is a separate verification concern.

use crate::config::ReviewerConfig;
use crate::llm::{self, LlmClient};
use anyhow::{Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// Built-in default prompt template, embedded at compile time so the binary
/// runs without requiring `prompts/` on the filesystem.
const DEFAULT_TEMPLATE: &str = include_str!("../../prompts/code-review-default.md");

/// Total cap (in chars) on the rendered prompt body — change context +
/// changed files + diff combined. Sized for modern 1M-token-class models
/// (Opus, Grok-4) at ~4 chars/token, conservatively halved. Individual
/// files are NEVER truncated; if a file's contents would push the total
/// over budget, the file is skipped in full and named in a footer.
const PROMPT_BUDGET: usize = 2_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewVerdict {
    Pass,
    Concerns,
    Block,
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

/// All the material the reviewer sees: the change(s) that shipped, the
/// resulting file state, and the unified diff. Rendering into a prompt
/// honors `PROMPT_BUDGET` in priority order (context > files > diff).
#[derive(Debug, Clone, Default)]
pub struct ReviewContext {
    pub archived_changes: Vec<ChangeBrief>,
    pub changed_files: Vec<ChangedFile>,
    pub diff: String,
}

pub struct CodeReviewer {
    client: Box<dyn LlmClient>,
    template: String,
    auto_revise_on_block: bool,
}

impl CodeReviewer {
    pub fn new(client: Box<dyn LlmClient>, template: String) -> Self {
        Self {
            client,
            template,
            auto_revise_on_block: false,
        }
    }

    /// Builder-style setter mirroring the config flag of the same name.
    /// The flag controls whether `Block`-verdict concerns get forwarded
    /// to the revision dispatcher as `<!-- reviewer-revision -->` PR
    /// comments. Default `false` (no behavioural change). Used by
    /// `from_config` to propagate `ReviewerConfig::auto_revise_on_block`
    /// onto the constructed reviewer; tests use it directly when they
    /// need the flag flipped without round-tripping a full config.
    pub fn with_auto_revise_on_block(mut self, enabled: bool) -> Self {
        self.auto_revise_on_block = enabled;
        self
    }

    /// Whether reviewer-initiated revisions are enabled for this
    /// reviewer instance. Read by the polling-loop posting step that
    /// turns `Block`-verdict concerns into `<!-- reviewer-revision -->`
    /// PR comments.
    pub fn auto_revise_on_block(&self) -> bool {
        self.auto_revise_on_block
    }

    /// Wire a reviewer from config: build the LLM client, load the prompt
    /// template (overridden or default).
    pub fn from_config(cfg: &ReviewerConfig) -> Result<Self> {
        let client = llm::build_from_config(cfg)?;
        let template = match &cfg.prompt_template_path {
            Some(path) => std::fs::read_to_string(path).with_context(|| {
                format!(
                    "reading reviewer prompt template at {}",
                    path.display()
                )
            })?,
            None => DEFAULT_TEMPLATE.to_string(),
        };
        Ok(Self::new(client, template).with_auto_revise_on_block(cfg.auto_revise_on_block))
    }

    pub async fn review(&self, context: &ReviewContext) -> Result<ReviewReport> {
        let rendered = render_sections(context);
        let prompt = self
            .template
            .replace("{{change_context}}", &rendered.change_context)
            .replace("{{changed_files}}", &rendered.changed_files)
            .replace("{{diff}}", &rendered.diff_or_explanation);
        log_prompt_stats(context, &rendered, prompt.len());
        let raw = self.client.complete(&prompt).await?;
        Ok(parse_response(&raw))
    }
}

/// Emit a single INFO log line describing the rendered prompt's shape:
/// per-section bytes, per-file bytes, total vs. budget, and any files
/// dropped due to budget exhaustion. Operators rely on this to tell at a
/// glance whether a review approached the prompt-budget cap.
fn log_prompt_stats(ctx: &ReviewContext, rendered: &RenderedSections, prompt_bytes: usize) {
    let file_sizes: String = ctx
        .changed_files
        .iter()
        .map(|f| format!("{}:{}", f.path, f.contents.len()))
        .collect::<Vec<_>>()
        .join(",");
    let file_bytes_total: usize = ctx.changed_files.iter().map(|f| f.contents.len()).sum();
    let pct = if PROMPT_BUDGET == 0 {
        0
    } else {
        (prompt_bytes.saturating_mul(100) / PROMPT_BUDGET).min(999)
    };
    tracing::info!(
        prompt_bytes = prompt_bytes,
        budget = PROMPT_BUDGET,
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

/// Rendered substitution values for the three template placeholders, sized
/// against `PROMPT_BUDGET` in priority order. Pure function for testability.
struct RenderedSections {
    change_context: String,
    changed_files: String,
    diff_or_explanation: String,
    /// Files whose contents were dropped to fit the budget. Empty when all
    /// files fit. Used by `review` to log a structured warning.
    skipped_files: Vec<String>,
}

fn render_sections(ctx: &ReviewContext) -> RenderedSections {
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
        if projected > PROMPT_BUDGET {
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
    //    rendered prompt past `PROMPT_BUDGET`.
    let used = change_context.len() + changed_files.len();
    let diff_or_explanation = if ctx.diff.is_empty() {
        String::from("(no diff produced this pass)")
    } else if !skipped.is_empty() || used + ctx.diff.len() > PROMPT_BUDGET {
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

    match (first_nonempty, found_idx) {
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
            }
        }
        _ => ReviewReport {
            verdict: ReviewVerdict::Concerns,
            markdown: format!(
                "[reviewer response did not include a valid verdict line]\n\n{raw}"
            ),
            concerns,
        },
    }
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
        let huge = "z".repeat(PROMPT_BUDGET + 100_000);
        let ctx = ReviewContext {
            archived_changes: Vec::new(),
            changed_files: vec![ChangedFile {
                path: "huge.rs".into(),
                contents: huge,
            }],
            diff: String::new(),
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
        };
        let r = render_sections(&ctx);
        assert!(r.change_context.contains("## Change: x"));
        assert!(r.change_context.contains("P\n\nD\n\nT"));
        assert!(r.changed_files.contains("## File: a.rs"));
        assert!(r.changed_files.contains("BODY"));
        assert_eq!(r.diff_or_explanation, "DELTA");
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
            provider: ReviewerProvider::Anthropic,
            model: "x".into(),
            api_key_env: Some("REVIEWER_TEST_KEY_OVERRIDE".into()),
            api_key: None,
            api_base_url: None,
            prompt_template_path: Some(template_path),
            auto_revise_on_block: false,
        };
        let reviewer = CodeReviewer::from_config(&cfg).expect("should load custom template");
        unsafe { std::env::remove_var("REVIEWER_TEST_KEY_OVERRIDE") };

        // The override must not match the default template's scope statement.
        assert!(
            !reviewer.template.contains("You are reviewing code quality only"),
            "user template should NOT contain the default's scope statement"
        );
        assert!(
            reviewer.template.contains("CUSTOM TEMPLATE:"),
            "user template should be the loaded file's contents"
        );
    }

    #[test]
    fn from_config_errors_when_template_path_missing() {
        use crate::config::{ReviewerConfig, ReviewerProvider};
        unsafe { std::env::set_var("REVIEWER_TEST_KEY_MISSING_TMPL", "k") };
        let bogus = std::path::PathBuf::from("/nonexistent/orchestrator-test-template.md");
        let cfg = ReviewerConfig {
            enabled: true,
            provider: ReviewerProvider::Anthropic,
            model: "x".into(),
            api_key_env: Some("REVIEWER_TEST_KEY_MISSING_TMPL".into()),
            api_key: None,
            api_base_url: None,
            prompt_template_path: Some(bogus.clone()),
            auto_revise_on_block: false,
        };
        let result = CodeReviewer::from_config(&cfg);
        let err = match result {
            Ok(_) => panic!("missing template must error"),
            Err(e) => e,
        };
        unsafe { std::env::remove_var("REVIEWER_TEST_KEY_MISSING_TMPL") };
        let msg = format!("{err:#}");
        assert!(
            msg.contains("/nonexistent/orchestrator-test-template.md"),
            "error must name the offending path; got: {msg}"
        );
    }

    #[test]
    fn from_config_uses_default_template_when_path_omitted() {
        use crate::config::{ReviewerConfig, ReviewerProvider};
        unsafe { std::env::set_var("REVIEWER_TEST_KEY_DEFAULT", "k") };
        let cfg = ReviewerConfig {
            enabled: true,
            provider: ReviewerProvider::Anthropic,
            model: "x".into(),
            api_key_env: Some("REVIEWER_TEST_KEY_DEFAULT".into()),
            api_key: None,
            api_base_url: None,
            prompt_template_path: None,
            auto_revise_on_block: false,
        };
        let reviewer = CodeReviewer::from_config(&cfg).expect("default template loads");
        unsafe { std::env::remove_var("REVIEWER_TEST_KEY_DEFAULT") };
        assert!(
            reviewer
                .template
                .contains("You are reviewing code quality only"),
            "default template must be used when prompt_template_path is None"
        );
    }

    #[test]
    fn default_template_describes_revision_requests_block() {
        // Operators who flip `reviewer.auto_revise_on_block` rely on the
        // default template producing the structured `revision-requests`
        // block. The template must instruct the LLM on it.
        assert!(
            DEFAULT_TEMPLATE.contains("revision-requests"),
            "default template must mention the `revision-requests` block name"
        );
        assert!(
            DEFAULT_TEMPLATE.contains("should_request_revision"),
            "default template must document the `should_request_revision` field"
        );
        assert!(
            DEFAULT_TEMPLATE.contains("actionable_request"),
            "default template must document the `actionable_request` field"
        );
        assert!(
            DEFAULT_TEMPLATE.to_lowercase().contains("most-critical-first")
                || DEFAULT_TEMPLATE.to_lowercase().contains("most critical first"),
            "default template must instruct on ordering for cap-budget truncation"
        );
    }

    #[test]
    fn default_template_contains_scope_statement_and_format() {
        // Architecture-baseline scenario: default template must contain the
        // literal scope statement AND specify the verdict format.
        assert!(
            DEFAULT_TEMPLATE.contains("You are reviewing code quality only. Do NOT assess whether the diff implements the spec; that is handled separately by the verifier step."),
            "default template must contain the exact scope statement"
        );
        assert!(
            DEFAULT_TEMPLATE.contains("VERDICT:"),
            "default template must instruct on verdict format"
        );
        // Rubric points enumerated.
        for rubric in &[
            "Security", "Error handling", "Naming", "style", "idioms",
            "Dead code", "bugs",
        ] {
            assert!(
                DEFAULT_TEMPLATE.to_lowercase().contains(&rubric.to_lowercase()),
                "default template missing rubric point `{rubric}`"
            );
        }
    }
}
