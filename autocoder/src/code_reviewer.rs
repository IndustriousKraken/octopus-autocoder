//! AI-driven code-quality reviewer. Sends a unified diff + change summary to
//! a configured LLM and parses the response into a structured
//! `ReviewReport`. Scope is deliberately code-quality only; spec compliance
//! is a separate verification concern handled by a future change.

use crate::config::ReviewerConfig;
use crate::llm::{self, LlmClient};
use anyhow::{Context, Result};
use regex::Regex;
use std::sync::OnceLock;

/// Built-in default prompt template, embedded at compile time so the binary
/// runs without requiring `prompts/` on the filesystem.
const DEFAULT_TEMPLATE: &str = include_str!("../../prompts/code-review-default.md");

/// Cap on diff length before substitution into the prompt. Reviewing more
/// than this within typical model context windows risks truncation by the
/// model; we truncate explicitly so the operator sees it in the report.
const DIFF_SIZE_BUDGET: usize = 100_000;

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
}

pub struct CodeReviewer {
    client: Box<dyn LlmClient>,
    template: String,
}

impl CodeReviewer {
    pub fn new(client: Box<dyn LlmClient>, template: String) -> Self {
        Self { client, template }
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
        Ok(Self::new(client, template))
    }

    pub async fn review(&self, diff: &str, change_summary: &str) -> Result<ReviewReport> {
        let diff_for_prompt = if diff.len() > DIFF_SIZE_BUDGET {
            // Char-boundary safe truncation: `truncate` panics on non-char
            // boundary, so build a `String` via chars iteration.
            let truncated: String = diff.chars().take(DIFF_SIZE_BUDGET).collect();
            format!("[diff truncated to 100k chars]\n{truncated}")
        } else {
            diff.to_string()
        };

        let prompt = self
            .template
            .replace("{{diff}}", &diff_for_prompt)
            .replace("{{change_summary}}", change_summary);
        let raw = self.client.complete(&prompt).await?;
        Ok(parse_response(&raw))
    }
}

/// Parse the LLM response into a `ReviewReport`. Per spec, the first
/// non-empty line MUST match `(?i)^VERDICT:\s*(Pass|Concerns|Block)\s*$`.
/// If matched, the rest of the response (after that line) is the
/// `markdown`. If unmatched, the verdict defaults to `Concerns` and a
/// parse-failure note is prepended.
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
            ReviewReport { verdict, markdown }
        }
        _ => ReviewReport {
            verdict: ReviewVerdict::Concerns,
            markdown: format!(
                "[reviewer response did not include a valid verdict line]\n\n{raw}"
            ),
        },
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

    #[tokio::test]
    async fn truncates_huge_diff() {
        let big_diff = "x".repeat(DIFF_SIZE_BUDGET + 5_000);
        let (client, captured) = stub_with_capture("VERDICT: Pass\n");
        let reviewer = CodeReviewer::new(client, "diff: {{diff}}".to_string());
        reviewer.review(&big_diff, "summary").await.unwrap();

        let prompt = captured.lock().unwrap().clone().unwrap();
        assert!(
            prompt.contains("[diff truncated to 100k chars]"),
            "truncation marker must be present in prompt"
        );
        let xs_count = prompt.matches('x').count();
        assert_eq!(
            xs_count, DIFF_SIZE_BUDGET,
            "expected exactly {DIFF_SIZE_BUDGET} x chars in prompt; got {xs_count}"
        );
    }

    #[tokio::test]
    async fn substitutes_template_variables() {
        let (client, captured) = stub_with_capture("VERDICT: Pass\n");
        let template = "summary={{change_summary}}\nDIFF<<<{{diff}}>>>".to_string();
        let reviewer = CodeReviewer::new(client, template);
        reviewer.review("the diff content", "my summary").await.unwrap();

        let prompt = captured.lock().unwrap().clone().unwrap();
        assert!(prompt.contains("summary=my summary"), "got: {prompt}");
        assert!(prompt.contains("DIFF<<<the diff content>>>"), "got: {prompt}");
    }

    #[tokio::test]
    async fn under_budget_diff_is_not_truncated() {
        let small_diff = "x".repeat(100);
        let (client, captured) = stub_with_capture("VERDICT: Pass\n");
        let reviewer = CodeReviewer::new(client, "{{diff}}".to_string());
        reviewer.review(&small_diff, "summary").await.unwrap();
        let prompt = captured.lock().unwrap().clone().unwrap();
        assert!(!prompt.contains("[diff truncated"), "small diff must not be truncated");
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
