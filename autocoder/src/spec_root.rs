//! OSS-fork support (a26): canonical-spec path resolver.
//!
//! Every site in the daemon that previously constructed paths under
//! `<code_workspace>/openspec/...` SHOULD route through `SpecRoot` so
//! that operators who set per-repo `spec_storage.path` can land their
//! canonical specs in an external git working tree (typically a
//! sibling repo they own) while autocoder continues to drive code
//! changes in the upstream fork.
//!
//! When `spec_storage` is unset (the default), `SpecRoot::spec_root_dir`
//! resolves to `<code_workspace>/openspec`, preserving the historical
//! behavior. When set, it resolves to `<spec_storage.path>/openspec`.

use crate::config::RepositoryConfig;
use std::path::{Path, PathBuf};

/// Resolver for canonical-spec paths under the configured root.
///
/// The module ships with the OSS-fork support (a26) config schema
/// AND tests. Call-site migration (the task-2.3 refactor of every
/// `<workspace>/openspec/...` literal) is deferred to follow-up
/// work; without it, `spec_storage` is config-only.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct SpecRoot {
    /// The code workspace's root (where source code is checked out).
    /// Retained even when `spec_storage` is configured so callers that
    /// need both roots (e.g. archive flows targeting different repos)
    /// can route appropriately.
    pub code_workspace: PathBuf,
    /// The directory containing `specs/`, `changes/`, and
    /// `changes/archive/`. When `spec_storage` is unset, this is
    /// `<code_workspace>/openspec`; when set, it is
    /// `<spec_storage.path>/openspec`.
    pub spec_root_dir: PathBuf,
    /// True when `spec_storage` was set on the repo — i.e., the spec
    /// root SHOULD be treated as an external git working tree distinct
    /// from the code workspace. The actual filesystem location may
    /// still resolve under the code workspace (workspace-relative
    /// `../my-specs`), so this field reflects the operator's intent
    /// rather than a path-prefix comparison.
    pub external: bool,
}

#[allow(dead_code)]
impl SpecRoot {
    /// Resolve the spec root for a repository given its code-workspace
    /// path. Consults `RepositoryConfig::resolved_spec_storage_dir` to
    /// pick between the default (workspace-internal) and external
    /// (operator-configured) location.
    pub fn for_repo(repo: &RepositoryConfig, code_workspace: &Path) -> Self {
        let (spec_root_dir, external) =
            match repo.resolved_spec_storage_dir(code_workspace) {
                Some(external) => (external.join("openspec"), true),
                None => (code_workspace.join("openspec"), false),
            };
        Self {
            code_workspace: code_workspace.to_path_buf(),
            spec_root_dir,
            external,
        }
    }

    /// Construct directly from a chosen `spec_root_dir`. Useful for
    /// tests and for callers that have already done the resolution.
    /// `external` marks the spec root as operator-configured (true) or
    /// workspace-internal default (false).
    pub fn from_parts(
        code_workspace: PathBuf,
        spec_root_dir: PathBuf,
        external: bool,
    ) -> Self {
        Self {
            code_workspace,
            spec_root_dir,
            external,
        }
    }

    /// `<spec_root>/specs` — canonical capability specs.
    pub fn canonical_specs_dir(&self) -> PathBuf {
        self.spec_root_dir.join("specs")
    }

    /// `<spec_root>/changes` — active (in-flight) changes.
    pub fn changes_dir(&self) -> PathBuf {
        self.spec_root_dir.join("changes")
    }

    /// `<spec_root>/changes/archive` — archived changes.
    pub fn archive_dir(&self) -> PathBuf {
        self.spec_root_dir.join("changes").join("archive")
    }

    /// True when the spec root is external to the code workspace
    /// (operator configured `spec_storage`).
    pub fn is_external(&self) -> bool {
        self.external
    }

    /// Working directory in which to invoke `openspec <subcommand>` —
    /// the parent of `openspec/`. Equivalent to `code_workspace` when
    /// not external; otherwise the operator-configured spec_storage
    /// path.
    pub fn openspec_cwd(&self) -> PathBuf {
        self.spec_root_dir
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| self.spec_root_dir.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{RepositoryConfig, SpecStorageConfig};

    fn fixture_repo() -> RepositoryConfig {
        RepositoryConfig { forge: None,
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
            octopus_guide: None,
            sandbox: None,
        }
    }

    #[test]
    fn resolver_returns_workspace_internal_paths_when_unset() {
        let ws = PathBuf::from("/tmp/ws/repo");
        let spec_root = SpecRoot::for_repo(&fixture_repo(), &ws);
        assert_eq!(spec_root.spec_root_dir, ws.join("openspec"));
        assert_eq!(spec_root.canonical_specs_dir(), ws.join("openspec/specs"));
        assert_eq!(spec_root.changes_dir(), ws.join("openspec/changes"));
        assert_eq!(
            spec_root.archive_dir(),
            ws.join("openspec/changes/archive")
        );
        assert!(!spec_root.is_external());
    }

    #[test]
    fn resolver_returns_external_paths_when_set_absolute() {
        let ws = PathBuf::from("/tmp/ws/repo");
        let mut repo = fixture_repo();
        repo.spec_storage = Some(SpecStorageConfig {
            path: "/abs/specs-repo".to_string(),
            push_remote: None,
            base_branch: None,
        });
        let spec_root = SpecRoot::for_repo(&repo, &ws);
        assert_eq!(
            spec_root.spec_root_dir,
            PathBuf::from("/abs/specs-repo/openspec")
        );
        assert_eq!(
            spec_root.canonical_specs_dir(),
            PathBuf::from("/abs/specs-repo/openspec/specs")
        );
        assert_eq!(
            spec_root.changes_dir(),
            PathBuf::from("/abs/specs-repo/openspec/changes")
        );
        assert_eq!(
            spec_root.archive_dir(),
            PathBuf::from("/abs/specs-repo/openspec/changes/archive")
        );
        assert!(spec_root.is_external());
    }

    #[test]
    fn resolver_returns_external_paths_when_set_relative() {
        let ws = PathBuf::from("/tmp/ws/repo");
        let mut repo = fixture_repo();
        repo.spec_storage = Some(SpecStorageConfig {
            path: "../my-specs".to_string(),
            push_remote: None,
            base_branch: None,
        });
        let spec_root = SpecRoot::for_repo(&repo, &ws);
        assert_eq!(
            spec_root.spec_root_dir,
            ws.join("../my-specs").join("openspec")
        );
        assert!(spec_root.is_external());
    }

    #[test]
    fn from_parts_preserves_paths_verbatim() {
        let ws = PathBuf::from("/code");
        let sr = PathBuf::from("/specs/openspec");
        let spec_root = SpecRoot::from_parts(ws.clone(), sr.clone(), true);
        assert_eq!(spec_root.code_workspace, ws);
        assert_eq!(spec_root.spec_root_dir, sr);
        assert!(spec_root.is_external());
    }
}
