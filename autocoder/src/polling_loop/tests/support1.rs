use super::*;

/// Write a self-heal-ready change into the fixture workspace: a proposal,
/// a tasks.md with every task `[x]`, and a spec under `specs/<cap>/` that
/// `openspec validate --strict` accepts. Commit it so the dirty check
/// stays clean.
pub(crate) fn add_committed_self_heal_change(
    workspace: &Path,
    name: &str,
    all_done: bool,
    valid_spec: bool,
) {
    let dir = workspace.join("openspec/changes").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("proposal.md"),
        "## Why\n\nfixture self-heal\n\n## What Changes\n\n- thing\n",
    )
    .unwrap();
    let tasks = if all_done {
        "- [x] 1.1 done\n- [x] 1.2 also done\n"
    } else {
        "- [x] 1.1 done\n- [ ] 1.2 still open\n"
    };
    std::fs::write(dir.join("tasks.md"), tasks).unwrap();
    let spec_dir = dir.join("specs").join("self-heal-fixture-cap");
    std::fs::create_dir_all(&spec_dir).unwrap();
    let spec_body = if valid_spec {
        "## ADDED Requirements\n\n### Requirement: Do thing\nThe system SHALL do the thing.\n\n#### Scenario: It works\n- **WHEN** triggered\n- **THEN** does thing\n"
    } else {
        // No scenario block → openspec validate --strict fails.
        "## ADDED Requirements\n\n### Requirement: Do thing\nThe system SHALL do the thing.\n"
    };
    std::fs::write(spec_dir.join("spec.md"), spec_body).unwrap();
    let st = std::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(workspace)
        .status()
        .unwrap();
    assert!(st.success());
    let st = std::process::Command::new("git")
        .args(["commit", "-q", "-m", &format!("scaffold {name}")])
        .current_dir(workspace)
        .status()
        .unwrap();
    assert!(st.success());
}

pub(crate) fn fixture_repo_for_rebuild_test() -> RepositoryConfig {
    RepositoryConfig { forge: None,
        url: "git@github.com:owner/repo.git".into(),
        local_path: None,
        base_branch: "main".into(),
        agent_branch: "agent-q".into(),
        poll_interval_sec: 60,
        chatops_channel_id: None,
        max_changes_per_pr: None,
        audits: None,
        spec_storage: None,
        upstream: None,
        auto_submit_pr: true,
        sandbox: None,
    }
}

pub(crate) fn make_rename_record(
    from: &str,
    to: &str,
    day: &str,
    summary: &str,
) -> crate::cli::sync_specs_deps::RenameRecord {
    crate::cli::sync_specs_deps::RenameRecord {
        from: from.into(),
        to: to.into(),
        day: day.into(),
        dependency_summary: summary.into(),
    }
}

/// Write a fixture archive entry with a known proposal.md.
pub(crate) fn write_fixture_archive(workspace: &Path, date_slug: &str, proposal: &str) {
    let dir = workspace.join("openspec/changes/archive").join(date_slug);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("proposal.md"), proposal).unwrap();
}

/// Write a fixture active-path proposal.md at
/// `<workspace>/openspec/changes/<change>/proposal.md`.
pub(crate) fn write_fixture_active_proposal(workspace: &Path, change: &str, proposal: &str) {
    let dir = workspace.join("openspec/changes").join(change);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("proposal.md"), proposal).unwrap();
}

/// Seed a dated archive entry for `change` at today's UTC date so a
/// subsequent `queue::archive(workspace, change)` would collide. The
/// path matches `queue::archive_collision_path` exactly.
pub(crate) fn pre_create_dated_archive_entry(workspace: &Path, change: &str) {
    let dated = format!("{}-{change}", chrono::Utc::now().format("%Y-%m-%d"));
    let archive_dir = workspace.join("openspec/changes/archive").join(&dated);
    std::fs::create_dir_all(&archive_dir).unwrap();
    std::fs::write(
        archive_dir.join("proposal.md"),
        "## Why\nprior archive entry from a merged PR\n",
    )
    .unwrap();
    // Commit so the workspace stays clean for the pre-pass dirty
    // check inside `run_pass_through_commits`.
    let run_git = |args: &[&str]| {
        let st = std::process::Command::new("git")
            .args(args)
            .current_dir(workspace)
            .status()
            .unwrap();
        assert!(st.success(), "git {args:?} failed in fixture pre-create");
    };
    run_git(&["add", "-A"]);
    run_git(&[
        "commit",
        "-q",
        "-m",
        &format!("seed archive entry for {change}"),
    ]);
}

pub(crate) fn make_report(verdict: ReviewVerdict, concerns: Vec<ReviewConcern>) -> ReviewReport {
    ReviewReport {
        verdict,
        markdown: "## Summary\nbase markdown.\n".to_string(),
        concerns,
        per_change_sections: Vec::new(),
        attribution: None,
    }
}

pub(crate) fn revisable_concern(summary: &str, request: &str) -> ReviewConcern {
    ReviewConcern {
        summary: summary.to_string(),
        actionable_request: Some(request.to_string()),
        should_request_revision: true,
        change_slug: None,
        ..Default::default()
    }
}

pub(crate) fn commentary_concern(summary: &str) -> ReviewConcern {
    ReviewConcern {
        summary: summary.to_string(),
        actionable_request: None,
        should_request_revision: false,
        change_slug: None,
        ..Default::default()
    }
}

/// Build a `CodeReviewer` whose `auto_revise` tri-state is `mode`, for
/// the gating tests below.
pub(crate) fn reviewer_with_auto_revise(mode: crate::config::AutoRevise) -> CodeReviewer {
    use crate::llm::LlmClient;
    use async_trait::async_trait;
    struct NoopClient;
    #[async_trait]
    impl LlmClient for NoopClient {
        async fn complete(&self, _: &str) -> Result<String> {
            Ok(String::new())
        }
    }
    CodeReviewer::new(Box::new(NoopClient), "t".to_string()).with_auto_revise(mode)
}

/// Build a minimal `CodeReviewer` whose `max_code_reviews_per_pr` is the
/// given value, for the PR-open state-init tests below.
pub(crate) fn reviewer_with_review_cap(cap: Option<u32>) -> CodeReviewer {
    use crate::llm::LlmClient;
    use async_trait::async_trait;
    struct NoopClient;
    #[async_trait]
    impl LlmClient for NoopClient {
        async fn complete(&self, _: &str) -> Result<String> {
            Ok(String::new())
        }
    }
    CodeReviewer::new(Box::new(NoopClient), "t".to_string()).with_max_code_reviews_per_pr(cap)
}
