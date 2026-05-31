//! a34: spec-storage commit + push + PR routing.
//!
//! When `spec_storage.path` is configured for a repo, a polling
//! iteration may produce spec changes that live in the spec_storage
//! repo's working tree rather than the code workspace. The helpers
//! here implement the canonical `orchestrator-cli` requirements:
//!
//!   - Classify the iteration's outcome as spec-only, code-only,
//!     dual-tree, OR clean by checking each tree's
//!     `git status --porcelain`.
//!   - Resolve the spec_storage repo's push remote AND PR base branch
//!     per the operator's `SpecStorageConfig` overrides (with the
//!     documented defaults: `"origin"` AND remote-tracked HEAD).
//!   - Parse `<owner>/<name>` from the spec_storage remote URL (SSH
//!     OR HTTPS form) for the `gh pr create --repo` equivalent.
//!   - Produce the `[specs] ` title prefix for spec-only AND
//!     dual-tree's spec PR.
//!
//! See `openspec/specs/orchestrator-cli/spec.md` AND
//! `openspec/specs/git-workflow-manager/spec.md`.

use crate::config::SpecStorageConfig;
use crate::git;
use anyhow::Result;
use std::path::Path;

/// Classification of a polling iteration's outcome based on the
/// uncommitted state of the code workspace AND (when configured) the
/// spec_storage working tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum IterationTreeClass {
    /// Code workspace dirty AND spec_storage clean (OR not configured).
    /// Existing canonical commit + push + PR flow applies.
    CodeOnly,
    /// Code workspace clean AND spec_storage dirty. The iteration's
    /// commit + push + PR step routes EXCLUSIVELY to the spec_storage
    /// repo's working tree.
    SpecOnly,
    /// Both trees dirty. Two PRs result: one against the code workspace
    /// repo, one against the spec_storage repo.
    DualTree,
    /// Both trees clean. No commit + push + PR happens.
    Clean,
}

/// Classify the iteration's outcome by probing the uncommitted state
/// of each tree. When `spec_storage_path` is `None`, the spec_storage
/// tree is treated as clean (the operator did not configure one).
#[allow(dead_code)]
pub fn classify_tree_state(
    code_workspace: &Path,
    spec_storage_path: Option<&Path>,
) -> Result<IterationTreeClass> {
    let code_dirty = !git::status_porcelain(code_workspace)?.is_empty();
    let spec_dirty = match spec_storage_path {
        Some(p) => !git::status_porcelain(p)?.is_empty(),
        None => false,
    };
    Ok(match (code_dirty, spec_dirty) {
        (true, false) => IterationTreeClass::CodeOnly,
        (false, true) => IterationTreeClass::SpecOnly,
        (true, true) => IterationTreeClass::DualTree,
        (false, false) => IterationTreeClass::Clean,
    })
}

/// Resolution of the spec_storage push remote per
/// `SpecStorageConfig.push_remote`: unset → `"origin"`; set → verbatim
/// (config-load already verified existence).
pub fn resolve_spec_storage_push_remote(ss: &SpecStorageConfig) -> String {
    ss.push_remote
        .as_deref()
        .map(|s| s.to_string())
        .unwrap_or_else(|| "origin".to_string())
}

/// Resolution of the spec_storage base branch per
/// `SpecStorageConfig.base_branch`: unset → query
/// `git -C <spec_storage.path> symbolic-ref refs/remotes/<remote>/HEAD`,
/// fall back to `"main"` on failure (per the canonical spec).
pub fn resolve_spec_storage_base_branch(
    ss: &SpecStorageConfig,
    spec_storage_path: &Path,
    push_remote: &str,
) -> String {
    if let Some(b) = ss.base_branch.as_deref() {
        return b.to_string();
    }
    match git::default_branch_for_remote(spec_storage_path, push_remote) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(
                spec_storage_path = %spec_storage_path.display(),
                remote = %push_remote,
                "spec_storage base-branch symbolic-ref query failed; falling back to `main`: {e:#}"
            );
            "main".to_string()
        }
    }
}

/// Parse `<owner>/<name>` from a git remote URL. Accepts both SSH
/// (`git@github.com:owner/name.git`) AND HTTPS
/// (`https://github.com/owner/name.git` or without `.git`) forms.
/// Returns `None` on parse failure; callers MAY log WARN AND fall
/// back to the code workspace's owner/name per the canonical spec.
pub fn parse_owner_repo_from_remote_url(url: &str) -> Option<(String, String)> {
    let trimmed = url.trim();
    // SSH form: `git@<host>:<owner>/<name>(.git)?`
    if let Some(after_colon) = trimmed
        .strip_prefix("git@")
        .and_then(|s| s.split_once(':').map(|(_h, rest)| rest))
    {
        return strip_dot_git_and_split(after_colon);
    }
    // HTTPS form: `https://<host>/<owner>/<name>(.git)?` (also
    // tolerates http:// and ssh://git@host/owner/name).
    for prefix in &["https://", "http://", "ssh://", "git://"] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            // Strip optional user@ prefix (e.g. ssh://git@github.com/...).
            let rest = rest.split_once('@').map(|(_u, r)| r).unwrap_or(rest);
            // After the host segment, the path is `/owner/name(.git)?`.
            let after_host = rest.split_once('/').map(|(_h, p)| p)?;
            return strip_dot_git_and_split(after_host);
        }
    }
    // Bare `owner/name` form as a last resort (e.g. `gh repo` shorthand).
    strip_dot_git_and_split(trimmed)
}

fn strip_dot_git_and_split(path: &str) -> Option<(String, String)> {
    let stripped = path.strip_suffix(".git").unwrap_or(path);
    let (owner, name) = stripped.split_once('/')?;
    if owner.is_empty() || name.is_empty() || name.contains('/') {
        return None;
    }
    Some((owner.to_string(), name.to_string()))
}

/// Apply the `[specs] ` PR title prefix when the iteration's outcome
/// is spec-only OR the dual-tree's spec half is being processed.
/// Code-only iterations AND the dual-tree's code half receive the
/// title verbatim.
#[allow(dead_code)]
pub fn apply_specs_title_prefix(title: &str, is_spec_pr: bool) -> String {
    if is_spec_pr {
        // Avoid double-prefixing if upstream code already added it.
        if title.starts_with("[specs] ") {
            title.to_string()
        } else {
            format!("[specs] {title}")
        }
    } else {
        title.to_string()
    }
}

/// Decide whether a PR's diff lives entirely under `openspec/`. Used
/// by the reviewer-skip-spec-only-prs gate AND by the post-iteration
/// title-prefix decision. `changed_paths` MUST be the file paths
/// touched by the iteration's commits (workspace-relative).
#[allow(dead_code)]
pub fn diff_is_spec_only(changed_paths: &[String]) -> bool {
    !changed_paths.is_empty()
        && changed_paths
            .iter()
            .all(|p| p.starts_with("openspec/"))
}

/// Resolved push/PR target for a spec-only (or dual-tree's spec half)
/// commit + push + PR step. Threaded through the
/// `stage_commit_push_spec_only` helper so callers don't re-resolve
/// mid-flow.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SpecStorageTarget {
    pub push_remote: String,
    pub base_branch: String,
    /// `<owner>/<name>` slug parsed from the spec_storage remote URL,
    /// OR `None` when the URL could not be parsed (callers MAY then
    /// fall back to the code workspace's owner/name per the canonical
    /// spec — degrades to opening the PR against the wrong repo;
    /// clearly visible to the operator).
    pub owner_repo_slug: Option<String>,
}

/// Resolve the spec-storage target end-to-end: push remote, base
/// branch, AND `<owner>/<name>` slug. Performs the per-iteration
/// resolution exactly once so the commit + push + PR steps share a
/// consistent view per the canonical "resolved values SHALL be
/// threaded through" requirement.
#[allow(dead_code)]
pub fn resolve_spec_storage_target(
    ss: &SpecStorageConfig,
    spec_storage_path: &Path,
) -> SpecStorageTarget {
    let push_remote = resolve_spec_storage_push_remote(ss);
    let base_branch =
        resolve_spec_storage_base_branch(ss, spec_storage_path, &push_remote);
    let owner_repo_slug = match remote_url_for(spec_storage_path, &push_remote) {
        Ok(url) => parse_owner_repo_from_remote_url(&url).map(|(o, n)| {
            format!("{o}/{n}")
        }),
        Err(e) => {
            tracing::warn!(
                spec_storage_path = %spec_storage_path.display(),
                remote = %push_remote,
                "spec_storage remote URL lookup failed; PR --repo will fall back to code workspace's owner/name: {e:#}"
            );
            None
        }
    };
    SpecStorageTarget {
        push_remote,
        base_branch,
        owner_repo_slug,
    }
}

/// Run `git -C <spec_storage_path> remote get-url <remote>` AND
/// return the URL on success. Wraps the raw git invocation so the
/// resolver above can react to failure (log + warn AND degrade).
fn remote_url_for(
    spec_storage_path: &Path,
    remote: &str,
) -> Result<String> {
    let out = std::process::Command::new("git")
        .args(["-C"])
        .arg(spec_storage_path)
        .args(["remote", "get-url", remote])
        .output()?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(anyhow::anyhow!(
            "git -C {} remote get-url {remote} failed: {stderr}",
            spec_storage_path.display()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// a34 §4.3: stage + commit + push the spec_storage working tree for
/// a spec-only (OR dual-tree's spec half) iteration. Returns the
/// new commit SHA on success. Caller is responsible for PR creation
/// via `github::create_pr` (or its test indirection) so this helper
/// stays HTTP-free AND trivially testable against a temp fixture.
#[allow(dead_code)]
pub fn stage_commit_push_spec_only(
    spec_storage_path: &Path,
    push_remote: &str,
    agent_branch: &str,
    commit_message: &str,
    force: bool,
) -> Result<String> {
    git::add_all(spec_storage_path)?;
    let sha = git::commit_in_tree(spec_storage_path, commit_message)?;
    git::push_in_tree(spec_storage_path, push_remote, agent_branch, force)?;
    Ok(sha)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ----- classify_tree_state -----

    fn init_git_repo(dir: &Path) {
        let st = std::process::Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(dir)
            .status()
            .unwrap();
        assert!(st.success());
        let st = std::process::Command::new("git")
            .args(["config", "user.email", "t@x"])
            .current_dir(dir)
            .status()
            .unwrap();
        assert!(st.success());
        let st = std::process::Command::new("git")
            .args(["config", "user.name", "t"])
            .current_dir(dir)
            .status()
            .unwrap();
        assert!(st.success());
        std::fs::write(dir.join("README.md"), "hi\n").unwrap();
        let st = std::process::Command::new("git")
            .args(["add", "README.md"])
            .current_dir(dir)
            .status()
            .unwrap();
        assert!(st.success());
        let st = std::process::Command::new("git")
            .args(["commit", "-q", "-m", "initial"])
            .current_dir(dir)
            .status()
            .unwrap();
        assert!(st.success());
    }

    #[test]
    fn classify_code_only_when_workspace_dirty_and_no_spec_storage() {
        let dir = tempfile::TempDir::new().unwrap();
        let ws = dir.path();
        init_git_repo(ws);
        std::fs::write(ws.join("new.txt"), "x").unwrap();
        let class = classify_tree_state(ws, None).unwrap();
        assert_eq!(class, IterationTreeClass::CodeOnly);
    }

    #[test]
    fn classify_spec_only_when_only_spec_storage_dirty() {
        let dir = tempfile::TempDir::new().unwrap();
        let ws = dir.path().join("ws");
        let ss = dir.path().join("ss");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::create_dir_all(&ss).unwrap();
        init_git_repo(&ws);
        init_git_repo(&ss);
        std::fs::write(ss.join("new-spec.md"), "x").unwrap();
        let class = classify_tree_state(&ws, Some(&ss)).unwrap();
        assert_eq!(class, IterationTreeClass::SpecOnly);
    }

    #[test]
    fn classify_dual_tree_when_both_dirty() {
        let dir = tempfile::TempDir::new().unwrap();
        let ws = dir.path().join("ws");
        let ss = dir.path().join("ss");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::create_dir_all(&ss).unwrap();
        init_git_repo(&ws);
        init_git_repo(&ss);
        std::fs::write(ws.join("code.rs"), "x").unwrap();
        std::fs::write(ss.join("spec.md"), "x").unwrap();
        let class = classify_tree_state(&ws, Some(&ss)).unwrap();
        assert_eq!(class, IterationTreeClass::DualTree);
    }

    #[test]
    fn classify_clean_when_both_clean() {
        let dir = tempfile::TempDir::new().unwrap();
        let ws = dir.path().join("ws");
        let ss = dir.path().join("ss");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::create_dir_all(&ss).unwrap();
        init_git_repo(&ws);
        init_git_repo(&ss);
        let class = classify_tree_state(&ws, Some(&ss)).unwrap();
        assert_eq!(class, IterationTreeClass::Clean);
    }

    // ----- resolve_spec_storage_push_remote / base_branch -----

    #[test]
    fn push_remote_defaults_to_origin_when_unset() {
        let ss = SpecStorageConfig {
            path: "irrelevant".to_string(),
            push_remote: None,
            base_branch: None,
        };
        assert_eq!(resolve_spec_storage_push_remote(&ss), "origin");
    }

    #[test]
    fn push_remote_uses_override_when_set() {
        let ss = SpecStorageConfig {
            path: "irrelevant".to_string(),
            push_remote: Some("upstream-fork".to_string()),
            base_branch: None,
        };
        assert_eq!(
            resolve_spec_storage_push_remote(&ss),
            "upstream-fork"
        );
    }

    #[test]
    fn base_branch_uses_override_when_set() {
        let ss = SpecStorageConfig {
            path: "irrelevant".to_string(),
            push_remote: None,
            base_branch: Some("develop".to_string()),
        };
        let resolved =
            resolve_spec_storage_base_branch(&ss, Path::new("/tmp/anything"), "origin");
        assert_eq!(resolved, "develop");
    }

    #[test]
    fn base_branch_falls_back_to_main_when_symref_unset() {
        let dir = tempfile::TempDir::new().unwrap();
        let ss_path = dir.path().join("ss");
        std::fs::create_dir_all(&ss_path).unwrap();
        init_git_repo(&ss_path);
        // The repo has no remotes; the symbolic-ref query fails AND we
        // fall back to `main`.
        let ss = SpecStorageConfig {
            path: ss_path.display().to_string(),
            push_remote: None,
            base_branch: None,
        };
        let resolved =
            resolve_spec_storage_base_branch(&ss, &ss_path, "origin");
        assert_eq!(resolved, "main");
    }

    // ----- parse_owner_repo_from_remote_url -----

    #[test]
    fn parse_ssh_form() {
        assert_eq!(
            parse_owner_repo_from_remote_url("git@github.com:speccorp/specs-repo.git"),
            Some(("speccorp".to_string(), "specs-repo".to_string()))
        );
        // Without .git.
        assert_eq!(
            parse_owner_repo_from_remote_url("git@github.com:speccorp/specs-repo"),
            Some(("speccorp".to_string(), "specs-repo".to_string()))
        );
    }

    #[test]
    fn parse_https_form() {
        assert_eq!(
            parse_owner_repo_from_remote_url(
                "https://github.com/speccorp/specs-repo.git"
            ),
            Some(("speccorp".to_string(), "specs-repo".to_string()))
        );
        assert_eq!(
            parse_owner_repo_from_remote_url("https://github.com/speccorp/specs-repo"),
            Some(("speccorp".to_string(), "specs-repo".to_string()))
        );
    }

    #[test]
    fn parse_ssh_uri_form() {
        assert_eq!(
            parse_owner_repo_from_remote_url(
                "ssh://git@github.com/speccorp/specs-repo.git"
            ),
            Some(("speccorp".to_string(), "specs-repo".to_string()))
        );
    }

    #[test]
    fn parse_rejects_malformed() {
        assert!(parse_owner_repo_from_remote_url("").is_none());
        assert!(parse_owner_repo_from_remote_url("not-a-url").is_none());
        assert!(parse_owner_repo_from_remote_url("git@host:owner").is_none());
    }

    // ----- apply_specs_title_prefix -----

    #[test]
    fn title_prefix_added_for_spec_pr() {
        let t = apply_specs_title_prefix("a36-brownfield-foo", true);
        assert_eq!(t, "[specs] a36-brownfield-foo");
    }

    #[test]
    fn title_prefix_omitted_for_code_pr() {
        let t = apply_specs_title_prefix("a40-fix-bar", false);
        assert_eq!(t, "a40-fix-bar");
    }

    #[test]
    fn title_prefix_idempotent_when_already_prefixed() {
        let t = apply_specs_title_prefix("[specs] a36-brownfield-foo", true);
        assert_eq!(t, "[specs] a36-brownfield-foo");
    }

    // ----- diff_is_spec_only -----

    #[test]
    fn diff_is_spec_only_true_when_all_openspec() {
        let paths = vec![
            "openspec/changes/a36/proposal.md".to_string(),
            "openspec/specs/foo/spec.md".to_string(),
        ];
        assert!(diff_is_spec_only(&paths));
    }

    #[test]
    fn diff_is_spec_only_false_when_mixed() {
        let paths = vec![
            "openspec/changes/a36/proposal.md".to_string(),
            "autocoder/src/foo.rs".to_string(),
        ];
        assert!(!diff_is_spec_only(&paths));
    }

    #[test]
    fn diff_is_spec_only_false_when_empty() {
        let paths: Vec<String> = Vec::new();
        assert!(!diff_is_spec_only(&paths));
    }

    // ----- a34 §7: end-to-end integration test -----

    /// Set up a working tree backed by a bare remote at `remote_path`.
    /// The working tree starts with an `initial` commit on `main`, AND
    /// has its remote-tracked HEAD pointing at `main` so
    /// `default_branch_for_remote` resolves cleanly.
    fn init_workspace_with_bare_remote(
        ws: &Path,
        remote_path: &Path,
        extra_remote: Option<(&str, &str)>,
    ) {
        std::fs::create_dir_all(ws).unwrap();
        let st = std::process::Command::new("git")
            .args(["init", "--bare", "-q", "-b", "main"])
            .arg(remote_path)
            .status()
            .unwrap();
        assert!(st.success(), "bare init failed");
        let st = std::process::Command::new("git")
            .args([
                "clone",
                "-q",
                remote_path.to_string_lossy().as_ref(),
                ws.to_string_lossy().as_ref(),
            ])
            .status()
            .unwrap();
        assert!(st.success(), "clone failed");
        let cfgs = [
            ("user.email", "t@x"),
            ("user.name", "t"),
        ];
        for (k, v) in cfgs {
            let st = std::process::Command::new("git")
                .args(["config", k, v])
                .current_dir(ws)
                .status()
                .unwrap();
            assert!(st.success());
        }
        std::fs::write(ws.join("README.md"), "x\n").unwrap();
        let st = std::process::Command::new("git")
            .args(["add", "README.md"])
            .current_dir(ws)
            .status()
            .unwrap();
        assert!(st.success());
        let st = std::process::Command::new("git")
            .args(["commit", "-q", "-m", "initial"])
            .current_dir(ws)
            .status()
            .unwrap();
        assert!(st.success());
        let st = std::process::Command::new("git")
            .args(["push", "-q", "-u", "origin", "main"])
            .current_dir(ws)
            .status()
            .unwrap();
        assert!(st.success());
        let st = std::process::Command::new("git")
            .args(["remote", "set-head", "origin", "main"])
            .current_dir(ws)
            .status()
            .unwrap();
        assert!(st.success());
        // Optionally add a SECOND remote (e.g. `github`) pointing at a
        // synthetic URL so the owner/repo parser can be exercised
        // without disturbing the file-path `origin` used for pushing.
        if let Some((remote_name, url)) = extra_remote {
            let st = std::process::Command::new("git")
                .args(["remote", "add", remote_name, url])
                .current_dir(ws)
                .status()
                .unwrap();
            assert!(st.success());
        }
    }

    /// a34 §7.1: end-to-end exercise of the spec-only commit + push +
    /// PR path. Constructs a code workspace + sibling spec_storage
    /// workspace, simulates a brownfield-draft executor by writing the
    /// change-directory artifacts into spec_storage's working tree,
    /// then drives the new helpers (classify, resolve, commit, push,
    /// create_pr-with-`--repo` via the mockito hook).
    ///
    /// Asserts (per the canonical task):
    ///   - The spec_storage workspace has ONE new commit.
    ///   - The code workspace has NO new commits.
    ///   - The spec_storage bare-repo's `agent_branch` ref matches the
    ///     new commit SHA.
    ///   - The captured PR `gh`-equivalent argv includes
    ///     `--repo <spec-owner>/<name>` (via the HTTP path component
    ///     `/repos/<spec-owner>/<name>/pulls`) AND
    ///     `--title "[specs] ..."`.
    #[tokio::test]
    async fn spec_only_commit_push_pr_end_to_end() {
        use crate::config::SpecStorageConfig;
        // Bind to a temp dir so cleanup is automatic.
        let scratch = tempfile::TempDir::new().unwrap();
        let code_ws = scratch.path().join("code-ws");
        let code_remote = scratch.path().join("code-remote.git");
        let spec_ws = scratch.path().join("spec-ws");
        let spec_remote = scratch.path().join("spec-remote.git");

        init_workspace_with_bare_remote(&code_ws, &code_remote, None);
        // Use a GitHub-shaped URL for the spec_storage's `github`
        // remote so the owner/repo parser produces `speccorp/specs-repo`
        // — while keeping `origin` pointing at the file-bare remote for
        // the actual push. Operators wanting the canonical `gh pr
        // create --repo` shape configure `spec_storage.push_remote:
        // "github"`.
        init_workspace_with_bare_remote(
            &spec_ws,
            &spec_remote,
            Some(("github", "git@github.com:speccorp/specs-repo.git")),
        );

        // Capture the code workspace HEAD before any spec-only work so
        // we can assert it didn't move.
        let code_head_before = crate::git::rev_parse(&code_ws, "HEAD")
            .expect("code-ws HEAD lookup succeeds");

        // Simulate a brownfield-draft executor by writing the change
        // artifacts into the spec_storage tree. The agent_branch is
        // also created so the push has a target.
        let agent_branch = "agent-q";
        let st = std::process::Command::new("git")
            .args(["checkout", "-q", "-b", agent_branch])
            .current_dir(&spec_ws)
            .status()
            .unwrap();
        assert!(st.success());
        let change_dir = spec_ws
            .join("openspec/changes")
            .join("brownfield-myfeature");
        std::fs::create_dir_all(change_dir.join("specs/myfeature")).unwrap();
        std::fs::write(
            change_dir.join("proposal.md"),
            "## Why\n\nBrownfield draft for `myfeature`.\n",
        )
        .unwrap();
        std::fs::write(change_dir.join("tasks.md"), "# Tasks\n\n- [ ] 1.1 ...\n").unwrap();
        std::fs::write(
            change_dir.join("specs/myfeature/spec.md"),
            "## ADDED Requirements\n\n### Requirement: foo\n\n#### Scenario: bar\n",
        )
        .unwrap();

        // ----- exercise the classification helper -----
        let class = classify_tree_state(&code_ws, Some(&spec_ws)).unwrap();
        assert_eq!(
            class,
            IterationTreeClass::SpecOnly,
            "expected SpecOnly, got {class:?}"
        );

        // ----- exercise the resolver against the `github` remote -----
        // The slug parser keys on the URL returned by
        // `git -C ... remote get-url <push_remote>`. We want the slug
        // to be `speccorp/specs-repo`, so the resolver must consult
        // the `github` remote (which we configured with the
        // GitHub-shaped URL). The actual push happens via `origin`
        // below — exercising both resolver inputs in one fixture.
        let ss_for_slug = SpecStorageConfig {
            path: spec_ws.display().to_string(),
            push_remote: Some("github".to_string()),
            base_branch: None,
        };
        let target_for_slug = resolve_spec_storage_target(&ss_for_slug, &spec_ws);
        assert_eq!(target_for_slug.push_remote, "github");
        assert_eq!(
            target_for_slug.owner_repo_slug.as_deref(),
            Some("speccorp/specs-repo"),
            "owner/repo parsed from github remote URL"
        );

        // Default-resolution path: unset `push_remote` → `origin`,
        // base_branch from remote-tracked HEAD = `main`.
        let ss_for_push = SpecStorageConfig {
            path: spec_ws.display().to_string(),
            push_remote: None,
            base_branch: None,
        };
        let target = resolve_spec_storage_target(&ss_for_push, &spec_ws);
        assert_eq!(target.push_remote, "origin");
        assert_eq!(target.base_branch, "main");

        // ----- exercise the stage + commit + push helper -----
        let commit_msg = "Brownfield draft: capability `myfeature`";
        let new_sha = stage_commit_push_spec_only(
            &spec_ws,
            &target.push_remote,
            agent_branch,
            commit_msg,
            true,
        )
        .expect("stage_commit_push_spec_only succeeds");
        assert_eq!(
            new_sha.len(),
            40,
            "commit SHA must be 40 chars: got {new_sha:?}"
        );

        // The spec_storage workspace has exactly ONE new commit on
        // agent_branch beyond main.
        let range = "main..agent-q".to_string();
        let count = crate::git::rev_list_count(&spec_ws, &range)
            .expect("rev-list count succeeds");
        assert_eq!(
            count, 1,
            "spec_storage agent_branch should have exactly 1 new commit: got {count}"
        );

        // The code workspace's HEAD must NOT have moved.
        let code_head_after = crate::git::rev_parse(&code_ws, "HEAD")
            .expect("code-ws HEAD lookup succeeds (post)");
        assert_eq!(
            code_head_before, code_head_after,
            "code workspace HEAD must remain unchanged"
        );

        // The spec_storage's bare-repo `agent_branch` ref now points at
        // `new_sha` (the push landed).
        let remote_sha_out = std::process::Command::new("git")
            .args(["rev-parse", agent_branch])
            .current_dir(&spec_remote)
            .output()
            .unwrap();
        assert!(remote_sha_out.status.success(), "rev-parse on remote failed");
        let remote_sha = String::from_utf8_lossy(&remote_sha_out.stdout)
            .trim()
            .to_string();
        assert_eq!(
            remote_sha, new_sha,
            "spec_storage bare remote's agent_branch must match new commit"
        );

        // ----- exercise create_pr with `--repo` via mockito -----
        // The HTTP path component `/repos/speccorp/specs-repo/pulls`
        // is the equivalent shape of `gh pr create --repo
        // speccorp/specs-repo`. We also assert the `title` carries
        // the `[specs] ` prefix.
        let mut server = mockito::Server::new_async().await;
        let title = apply_specs_title_prefix("brownfield-myfeature", true);
        assert_eq!(
            title, "[specs] brownfield-myfeature",
            "spec-only PR title must carry the [specs] prefix"
        );

        let pr_mock = server
            .mock("POST", "/repos/speccorp/specs-repo/pulls")
            .match_body(mockito::Matcher::PartialJsonString(
                format!(r#"{{"title":"{title}","base":"main"}}"#)
            ))
            .with_status(201)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"html_url":"https://github.com/speccorp/specs-repo/pull/42","number":42}"#,
            )
            .create_async()
            .await;
        // Belt-and-suspenders: also fail loudly if the code workspace
        // repo's `/pulls` endpoint is hit (mis-routing detection).
        let _code_mock = server
            .mock("POST", "/repos/some/other-repo/pulls")
            .with_status(201)
            .expect(0)
            .create_async()
            .await;

        let pr = crate::github::create_pr_at_for_test(
            &server.url(),
            "some",
            "other-repo",
            target_for_slug.owner_repo_slug.as_deref(),
            agent_branch,
            &target.base_branch,
            &title,
            "(body)",
            "testtoken",
            None,
            false,
        )
        .await
        .expect("create_pr should succeed against speccorp/specs-repo");

        assert_eq!(
            pr.html_url,
            "https://github.com/speccorp/specs-repo/pull/42"
        );
        assert_eq!(pr.number, 42);
        pr_mock.assert_async().await;
    }

    /// a34 §5.4: dual-tree iteration produces TWO PRs — the spec PR
    /// prefixed `[specs] `, the code PR unprefixed. This unit-tests
    /// the title-prefix construction logic for both halves.
    #[test]
    fn dual_tree_title_prefix_per_half() {
        let code_title = apply_specs_title_prefix("a42-mixed-baz", false);
        let spec_title = apply_specs_title_prefix("a42-mixed-baz", true);
        assert_eq!(code_title, "a42-mixed-baz");
        assert_eq!(spec_title, "[specs] a42-mixed-baz");
    }
}
