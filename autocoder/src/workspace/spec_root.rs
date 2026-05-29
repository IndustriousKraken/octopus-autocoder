//! Spec-root resolver (a26): single source of truth for where canonical
//! specs live for a given repository. The default places them inside the
//! code workspace at `<workspace>/openspec/`; when a per-repo
//! `spec_storage.path` block is configured, the resolver redirects all
//! spec reads AND writes to `<spec_storage.path>/openspec/`.
//!
//! Every call site that previously composed `<workspace>/openspec/...`
//! SHOULD construct a [`SpecRoot`] AND use its methods so the
//! single-config-flag flip propagates uniformly.

use crate::config::{RepositoryConfig, SpecStorageConfig};
use std::path::{Path, PathBuf};

/// Resolved spec-root locations for one repository.
///
/// `code_workspace` is always the repository's code-workspace path
/// (where source files live). `spec_root_dir` is the directory that
/// contains the `openspec/` tree — either equal to `code_workspace`
/// (default) OR the configured `spec_storage.path` (when set).
///
/// The `spec_storage_configured` flag distinguishes "specs live in a
/// SEPARATE git working tree" (callers may need to commit/push there
/// instead of in the code workspace) from "specs live inside the code
/// workspace" (the historical default).
#[derive(Debug, Clone)]
pub struct SpecRoot {
    pub code_workspace: PathBuf,
    /// The directory containing `openspec/` — typically `code_workspace`
    /// (default) OR the external `spec_storage.path` value when
    /// configured. NOT the `openspec/` directory itself; the
    /// [`canonical_specs_dir`], [`changes_dir`], AND [`archive_dir`]
    /// helpers compose the standard subpaths.
    pub spec_root_dir: PathBuf,
    pub spec_storage_configured: bool,
}

impl SpecRoot {
    /// Resolve the spec-root for a repository. `code_workspace` is the
    /// repo's resolved workspace path (per `crate::workspace::resolve_path`).
    /// When `repo.spec_storage` is set, the resolver redirects
    /// `spec_root_dir` to the configured external path; otherwise it
    /// equals `code_workspace`.
    pub fn for_repo(repo: &RepositoryConfig, code_workspace: PathBuf) -> Self {
        match repo.spec_storage.as_ref() {
            Some(ss) => {
                let raw = PathBuf::from(&ss.path);
                let spec_root_dir = if raw.is_absolute() {
                    raw
                } else {
                    code_workspace.join(raw)
                };
                Self {
                    code_workspace,
                    spec_root_dir,
                    spec_storage_configured: true,
                }
            }
            None => Self {
                code_workspace: code_workspace.clone(),
                spec_root_dir: code_workspace,
                spec_storage_configured: false,
            },
        }
    }

    /// Direct constructor for tests AND non-config call sites. Marks
    /// `spec_storage_configured` based on whether `spec_root_dir`
    /// differs from `code_workspace`.
    #[allow(dead_code)]
    pub fn from_paths(code_workspace: PathBuf, spec_root_dir: PathBuf) -> Self {
        let spec_storage_configured = spec_root_dir != code_workspace;
        Self {
            code_workspace,
            spec_root_dir,
            spec_storage_configured,
        }
    }

    /// `<spec_root_dir>/openspec/` — the directory the OpenSpec CLI
    /// treats as its repo root (contains `specs/`, `changes/`, AND
    /// `changes/archive/`).
    pub fn openspec_dir(&self) -> PathBuf {
        self.spec_root_dir.join("openspec")
    }

    /// `<spec_root_dir>/openspec/specs/` — canonical specs root.
    pub fn canonical_specs_dir(&self) -> PathBuf {
        self.openspec_dir().join("specs")
    }

    /// `<spec_root_dir>/openspec/changes/` — proposed (in-flight)
    /// change directory.
    pub fn changes_dir(&self) -> PathBuf {
        self.openspec_dir().join("changes")
    }

    /// `<spec_root_dir>/openspec/changes/archive/` — archived change
    /// directory.
    pub fn archive_dir(&self) -> PathBuf {
        self.changes_dir().join("archive")
    }

    /// True when spec reads AND writes route to an EXTERNAL working
    /// tree (i.e., `spec_storage` is configured). Callers that commit
    /// spec changes SHOULD branch on this to decide which git working
    /// tree to operate against.
    pub fn is_external(&self) -> bool {
        self.spec_storage_configured
    }

    /// The git working tree that holds the spec files — equals
    /// [`spec_root_dir`] when `spec_storage` is configured, otherwise
    /// [`code_workspace`].
    pub fn spec_git_workspace(&self) -> &Path {
        &self.spec_root_dir
    }
}

// --------------------------------------------------------------------------
// Module-level convenience helpers. Use these at call sites that have
// `&RepositoryConfig` AND `&Path` (workspace) but don't want to
// materialize an owned `SpecRoot` every time.
// --------------------------------------------------------------------------

/// `<spec_root>/openspec/specs/` for `repo` given its resolved
/// `code_workspace`. Equivalent to `SpecRoot::for_repo(repo, ws).canonical_specs_dir()`.
pub fn specs_dir(repo: &RepositoryConfig, code_workspace: &Path) -> PathBuf {
    SpecRoot::for_repo(repo, code_workspace.to_path_buf()).canonical_specs_dir()
}

/// `<spec_root>/openspec/changes/` for `repo`.
pub fn changes_dir(repo: &RepositoryConfig, code_workspace: &Path) -> PathBuf {
    SpecRoot::for_repo(repo, code_workspace.to_path_buf()).changes_dir()
}

/// `<spec_root>/openspec/changes/archive/` for `repo`.
pub fn archive_dir(repo: &RepositoryConfig, code_workspace: &Path) -> PathBuf {
    SpecRoot::for_repo(repo, code_workspace.to_path_buf()).archive_dir()
}

/// `<spec_root>/openspec/` for `repo`.
pub fn openspec_dir(repo: &RepositoryConfig, code_workspace: &Path) -> PathBuf {
    SpecRoot::for_repo(repo, code_workspace.to_path_buf()).openspec_dir()
}

/// The git working tree that holds spec files — either the code
/// workspace (default) OR the configured `spec_storage.path` (external).
pub fn spec_git_workspace(repo: &RepositoryConfig, code_workspace: &Path) -> PathBuf {
    SpecRoot::for_repo(repo, code_workspace.to_path_buf())
        .spec_git_workspace()
        .to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_repo(spec_storage: Option<SpecStorageConfig>) -> RepositoryConfig {
        RepositoryConfig {
            url: "git@github.com:owner/repo.git".to_string(),
            local_path: None,
            base_branch: "main".to_string(),
            agent_branch: "agent-q".to_string(),
            poll_interval_sec: 60,
            chatops_channel_id: None,
            max_changes_per_pr: None,
            audits: None,
            spec_storage,
            upstream: None,
            auto_submit_pr: true,
        }
    }

    #[test]
    fn default_resolves_under_workspace() {
        let workspace = PathBuf::from("/tmp/ws");
        let repo = make_repo(None);
        let root = SpecRoot::for_repo(&repo, workspace.clone());
        assert!(!root.is_external());
        assert_eq!(root.openspec_dir(), workspace.join("openspec"));
        assert_eq!(
            root.canonical_specs_dir(),
            workspace.join("openspec/specs")
        );
        assert_eq!(root.changes_dir(), workspace.join("openspec/changes"));
        assert_eq!(
            root.archive_dir(),
            workspace.join("openspec/changes/archive")
        );
    }

    #[test]
    fn spec_storage_absolute_redirects() {
        let workspace = PathBuf::from("/tmp/ws");
        let repo = make_repo(Some(SpecStorageConfig {
            path: "/abs/path/specs".to_string(),
        }));
        let root = SpecRoot::for_repo(&repo, workspace);
        assert!(root.is_external());
        assert_eq!(
            root.openspec_dir(),
            PathBuf::from("/abs/path/specs/openspec")
        );
        assert_eq!(
            root.canonical_specs_dir(),
            PathBuf::from("/abs/path/specs/openspec/specs")
        );
    }

    #[test]
    fn spec_storage_relative_resolves_under_workspace() {
        let workspace = PathBuf::from("/tmp/ws");
        let repo = make_repo(Some(SpecStorageConfig {
            path: "../my-specs".to_string(),
        }));
        let root = SpecRoot::for_repo(&repo, workspace.clone());
        assert!(root.is_external());
        // Relative path joins under the resolved workspace path.
        assert_eq!(
            root.openspec_dir(),
            workspace.join("../my-specs").join("openspec")
        );
    }
}
