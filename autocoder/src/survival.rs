//! Survival analysis (`survives`) AND provenance lookup (`blame`) —
//! read-only git-history analysis for reviewing past work.
//!
//! Survival answers: of what a past PR or commit changed, how much is
//! still live at `HEAD`? An operator can then review long-past, untrusted
//! work that is STILL present AND spec a fix only for the surviving
//! problems. Provenance is the inverse: a problem found in current code is
//! traced to the commit (and PR) that introduced it.
//!
//! ## The verbatim-vs-semantic boundary (load-bearing)
//!
//! Survival is computed with `git blame`, which attributes a line to the
//! LAST commit that touched it. So a line the target introduced that a
//! later commit reformatted, renamed, or moved attributes to the NEWER
//! commit AND is reported as NOT surviving — even though its substance
//! persists. The analysis therefore:
//!
//! - UNDER-reports survival (it may miss surviving-but-edited lines), AND
//! - NEVER over-reports (a line reported as surviving is the target's
//!   EXACT text).
//!
//! This boundary is stated in the rendered report ([`SurvivalReport::render`]),
//! not just here, so "N lines survive" is read correctly. `git blame -M
//! -C` may be applied to recover relocated lines; it is heuristic AND does
//! not change the boundary.
//!
//! Everything here is read-only: it runs `git log`, `git rev-list`, `git
//! show`, AND `git blame` only. It moves no branch, touches no workspace
//! file, AND writes no marker, archive, or canon.

use crate::git::{self, BlameLine};
use anyhow::Result;
use std::collections::BTreeSet;
use std::path::Path;

/// What the operator named as the survival/blame target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurvivalTarget {
    /// A single commit (full or short SHA).
    Commit { sha: String },
    /// A pull request, resolved to its commit-set against the base branch.
    Pr { number: u64 },
}

impl SurvivalTarget {
    /// A short human label for the report header.
    pub fn label(&self) -> String {
        match self {
            SurvivalTarget::Commit { sha } => format!("commit {sha}"),
            SurvivalTarget::Pr { number } => format!("PR #{number}"),
        }
    }
}

/// Survival status for one file the target modified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileSurvival {
    /// No commit after the target touched this file (cheap pre-filter):
    /// every line the target wrote there is still present, no blame run.
    FullyViaPreFilter,
    /// A later commit touched the file; line-level blame found the listed
    /// surviving line regions still attributing to the target. `surviving`
    /// is the count of still-attributed lines; `target_lines` is the count
    /// of lines the file currently holds that the target could have
    /// written (here, the lines blamed at HEAD that the file contains).
    Partial {
        regions: Vec<LineRegion>,
        surviving: usize,
    },
    /// A later commit overwrote every line the target wrote here: nothing
    /// of the target survives in this file.
    None,
}

/// A contiguous run of surviving line numbers `[start, end]` (1-based,
/// inclusive) at `HEAD`. Consumable as a review focus: `files <path>` plus
/// these regions tell the on-demand reviewer exactly what is still live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineRegion {
    pub start: usize,
    pub end: usize,
}

/// Per-file survival entry in the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileReport {
    pub path: String,
    pub survival: FileSurvival,
}

/// The full survival report for a target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurvivalReport {
    pub target_label: String,
    /// The resolved commit-set the target maps to (full SHAs). Empty is an
    /// error condition handled before report construction.
    pub commit_shas: Vec<String>,
    pub files: Vec<FileReport>,
    /// Whether move/copy detection (`-M -C`) was applied.
    pub detect_moves: bool,
}

impl SurvivalReport {
    /// Total surviving lines across all files. A pre-filtered file counts
    /// its blamed-at-HEAD lines (every line the target wrote there is
    /// present); a partial file counts its surviving region lines.
    pub fn total_surviving_lines(&self) -> usize {
        self.files
            .iter()
            .map(|f| match &f.survival {
                FileSurvival::FullyViaPreFilter => 0, // counted via pre_filter_lines
                FileSurvival::Partial { surviving, .. } => *surviving,
                FileSurvival::None => 0,
            })
            .sum()
    }

    /// Count of files that fully survive (pre-filter).
    pub fn fully_surviving_files(&self) -> usize {
        self.files
            .iter()
            .filter(|f| matches!(f.survival, FileSurvival::FullyViaPreFilter))
            .count()
    }

    /// Count of files that partially survive.
    pub fn partial_files(&self) -> usize {
        self.files
            .iter()
            .filter(|f| matches!(f.survival, FileSurvival::Partial { .. }))
            .count()
    }

    /// Count of files where nothing of the target survives.
    pub fn gone_files(&self) -> usize {
        self.files
            .iter()
            .filter(|f| matches!(f.survival, FileSurvival::None))
            .count()
    }

    /// Render the report to a plain-text block (chatops reply / CLI
    /// stdout). States the verbatim-survival boundary plainly so "N lines
    /// survive" is read correctly, names the target AND its resolved
    /// commit-set, lists per-file survival WITH surviving line regions, AND
    /// closes with the overall counts.
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("Survival of {} at HEAD\n", self.target_label));
        let shas: Vec<String> = self.commit_shas.iter().map(|s| short(s)).collect();
        out.push_str(&format!("  resolved commit-set: {}\n", shas.join(", ")));
        if self.detect_moves {
            out.push_str("  move/copy detection: on (-M -C)\n");
        }
        out.push('\n');

        if self.files.is_empty() {
            out.push_str("  (the target modified no files, or none could be resolved)\n\n");
        }
        for f in &self.files {
            match &f.survival {
                FileSurvival::FullyViaPreFilter => {
                    out.push_str(&format!(
                        "  ✓ {}\n      fully surviving (untouched since the target; no blame run)\n",
                        f.path
                    ));
                }
                FileSurvival::Partial { regions, surviving } => {
                    out.push_str(&format!(
                        "  ~ {}\n      partially surviving: {surviving} line(s) still attribute to the target\n",
                        f.path
                    ));
                    out.push_str(&format!("      surviving regions: {}\n", render_regions(regions)));
                }
                FileSurvival::None => {
                    out.push_str(&format!(
                        "  ✗ {}\n      not surviving (every line the target wrote here was later overwritten)\n",
                        f.path
                    ));
                }
            }
        }
        out.push('\n');
        out.push_str(&format!(
            "Overall: {} file(s) fully surviving, {} partially, {} gone; \
             {} line(s) still attribute to the target across partially-surviving files.\n",
            self.fully_surviving_files(),
            self.partial_files(),
            self.gone_files(),
            self.total_surviving_lines(),
        ));
        // The boundary statement — REQUIRED in the output, not just code
        // comments. It tells the operator how to read the counts.
        out.push_str(
            "\nBoundary: this detects VERBATIM survival, not semantic survival. \
             `git blame` attributes a line to the LAST commit that touched it, so a line the \
             target introduced that was later reformatted, renamed, or moved attributes to the \
             newer commit AND is reported as NOT surviving even if its substance persists. \
             This analysis therefore UNDER-reports survival (it may miss surviving-but-edited \
             lines) AND never over-reports — a line reported as surviving is the target's exact \
             text. Move/copy detection (-M -C) may recover some relocated lines; it is heuristic \
             AND does not change this boundary.\n",
        );
        // Make the surviving regions consumable as a review focus.
        let review_targets = self.review_focus_paths();
        if !review_targets.is_empty() {
            out.push_str(
                "\nReview only what is still live — surviving files (regions noted above):\n",
            );
            for p in &review_targets {
                out.push_str(&format!("  files {p}\n"));
            }
        }
        out
    }

    /// The workspace-relative paths that still hold target content (fully
    /// OR partially surviving). Returned so the operator can follow up with
    /// `review <repo> files <path...>` scoped to still-live code. A file
    /// where nothing survives is omitted.
    pub fn review_focus_paths(&self) -> Vec<String> {
        self.files
            .iter()
            .filter(|f| !matches!(f.survival, FileSurvival::None))
            .map(|f| f.path.clone())
            .collect()
    }
}

/// Short-SHA (7 chars) for display; passes through anything shorter.
fn short(sha: &str) -> String {
    if sha.len() > 7 {
        sha[..7].to_string()
    } else {
        sha.to_string()
    }
}

/// Render a region list `[a-b, c, d-e]` compactly. A single-line region
/// renders as the bare number.
fn render_regions(regions: &[LineRegion]) -> String {
    if regions.is_empty() {
        return "(none)".to_string();
    }
    regions
        .iter()
        .map(|r| {
            if r.start == r.end {
                r.start.to_string()
            } else {
                format!("{}-{}", r.start, r.end)
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Collapse a sorted set of 1-based line numbers into contiguous
/// inclusive [`LineRegion`]s. Pure; exposed for unit-testing.
pub(crate) fn lines_to_regions(lines: &BTreeSet<usize>) -> Vec<LineRegion> {
    let mut out: Vec<LineRegion> = Vec::new();
    let mut iter = lines.iter().copied();
    let Some(first) = iter.next() else {
        return out;
    };
    let mut start = first;
    let mut prev = first;
    for n in iter {
        if n == prev + 1 {
            prev = n;
        } else {
            out.push(LineRegion { start, end: prev });
            start = n;
            prev = n;
        }
    }
    out.push(LineRegion { start, end: prev });
    out
}

/// Resolve a [`SurvivalTarget`] into its commit-set (full SHAs) against
/// the workspace's base branch. A `Commit` target resolves to that single
/// commit's full SHA. A `Pr` target resolves to BOTH the squash/merge
/// commit(s) named on the base branch (so a squashed PR's surviving lines
/// blame to the squash commit) AND the PR head's own commits fetched into
/// a local ref (so a merge-without-squash PR's surviving lines blame to
/// the originals). Read-only; the only network step is the best-effort PR
/// head fetch, whose failure is tolerated when base-branch squash commits
/// were found.
pub fn resolve_target_commits(
    workspace: &Path,
    base_branch: &str,
    remote: &str,
    target: &SurvivalTarget,
) -> Result<Vec<String>> {
    match target {
        SurvivalTarget::Commit { sha } => {
            let full = git::rev_parse(workspace, sha)?;
            Ok(vec![full])
        }
        SurvivalTarget::Pr { number } => {
            let mut shas: Vec<String> = Vec::new();
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
            // Squash / merge commit(s) on the base branch by subject.
            for s in git::base_commits_for_pr(workspace, base_branch, *number)? {
                if seen.insert(s.clone()) {
                    shas.push(s);
                }
            }
            // The PR head's own commits (covers a rebase/merge-without-squash
            // PR). Best-effort: if the PR ref cannot be fetched AND we already
            // have base-branch squash commits, proceed with those.
            match git::fetch_pull_request_head(workspace, remote, *number) {
                Ok(head_ref) => {
                    let range = format!("{base_branch}..{head_ref}");
                    if let Ok(head_commits) = git::rev_list_range(workspace, &range) {
                        for s in head_commits {
                            if seen.insert(s.clone()) {
                                shas.push(s);
                            }
                        }
                    }
                }
                Err(e) => {
                    if shas.is_empty() {
                        return Err(anyhow::anyhow!(
                            "could not resolve PR #{number}: no base-branch commit names it \
                             (squash/merge subject) AND fetching its head ref failed: {e:#}"
                        ));
                    }
                    tracing::debug!(
                        pr = number,
                        "survival: PR head fetch failed but base-branch squash commit found: {e:#}"
                    );
                }
            }
            if shas.is_empty() {
                return Err(anyhow::anyhow!(
                    "PR #{number} resolved to no commits (no base-branch squash/merge subject \
                     names it AND its head ref carried no commits beyond {base_branch})"
                ));
            }
            Ok(shas)
        }
    }
}

/// Compute the survival report for `target` at `HEAD`.
///
/// For each file the target modified (via `git show --name-status`):
///  - the cheap pre-filter `git log <target>..HEAD -- <file>`: when EMPTY
///    (no later commit touched the file), the file is reported fully
///    surviving WITHOUT line-level blame;
///  - otherwise `git blame` at `HEAD` over the file, keeping the lines
///    whose blame-commit is in the target's commit-set (still attribute to
///    the target), collapsed into surviving regions; an all-overwritten
///    file is reported as not surviving.
///
/// The pre-filter is run against the FIRST commit of the target set (the
/// oldest by position is unimportant — for a single commit it is the
/// commit; for a PR the `<target>..HEAD` range from any of its commits
/// over-approximates "touched since", which is safe: a false "touched"
/// only forces a blame that confirms full survival anyway). To keep the
/// pre-filter exact for the common single-commit case AND conservative for
/// PRs, the pre-filter uses each target commit AND reports fully-surviving
/// only when NO target commit shows a later touch.
///
/// Read-only.
pub fn analyze_survival(
    workspace: &Path,
    base_branch: &str,
    remote: &str,
    target: &SurvivalTarget,
    detect_moves: bool,
) -> Result<SurvivalReport> {
    let commit_shas = resolve_target_commits(workspace, base_branch, remote, target)?;
    let target_set: std::collections::HashSet<&str> =
        commit_shas.iter().map(String::as_str).collect();

    let files = git::files_modified_by_commits(workspace, &commit_shas)?;
    let mut file_reports: Vec<FileReport> = Vec::new();
    for path in files {
        // Pre-filter: a file no later commit touched (relative to EVERY
        // target commit) fully survives without blame.
        let touched_since = commit_shas
            .iter()
            .map(|sha| git::file_touched_since(workspace, sha, &path))
            .collect::<Result<Vec<bool>>>()?;
        let any_later_touch = touched_since.iter().any(|&t| t);
        if !any_later_touch {
            file_reports.push(FileReport {
                path,
                survival: FileSurvival::FullyViaPreFilter,
            });
            continue;
        }

        // Line-level: blame at HEAD; keep lines whose commit is in the
        // target set. A file that no longer exists at HEAD (deleted by a
        // later commit) blames-empty → not surviving.
        let blamed: Vec<BlameLine> =
            match git::blame_lines(workspace, "HEAD", &path, None, detect_moves) {
                Ok(b) => b,
                Err(_) => {
                    // The file does not exist at HEAD anymore (or is
                    // unblameable): nothing of the target survives there.
                    file_reports.push(FileReport {
                        path,
                        survival: FileSurvival::None,
                    });
                    continue;
                }
            };
        let surviving_lines: BTreeSet<usize> = blamed
            .iter()
            .filter(|b| target_set.contains(b.sha.as_str()))
            .map(|b| b.line_no)
            .collect();
        if surviving_lines.is_empty() {
            file_reports.push(FileReport {
                path,
                survival: FileSurvival::None,
            });
        } else {
            let surviving = surviving_lines.len();
            let regions = lines_to_regions(&surviving_lines);
            file_reports.push(FileReport {
                path,
                survival: FileSurvival::Partial { regions, surviving },
            });
        }
    }

    Ok(SurvivalReport {
        target_label: target.label(),
        commit_shas,
        files: file_reports,
        detect_moves,
    })
}

// ====================================================================
// Provenance lookup
// ====================================================================

/// One blamed line's provenance: the introducing commit AND, when the
/// commit's subject names a PR (GitHub squash/merge convention), that PR
/// number. `pr` is `None` when no PR association is found — the commit is
/// then reported alone, NEVER a fabricated PR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvenanceLine {
    pub line_no: usize,
    pub short_sha: String,
    pub subject: String,
    pub date: String,
    pub pr: Option<u64>,
}

/// The provenance report for a file's line(s).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvenanceReport {
    pub path: String,
    pub lines: Vec<ProvenanceLine>,
}

impl ProvenanceReport {
    /// Render the provenance report to a plain-text block. One line per
    /// blamed line: number, short SHA, date, subject, AND the PR when one
    /// is associated. No PR association → the commit alone (no fabricated
    /// PR).
    pub fn render(&self) -> String {
        let mut out = format!("Provenance of `{}` at HEAD:\n", self.path);
        if self.lines.is_empty() {
            out.push_str("  (no lines blamed)\n");
        }
        for l in &self.lines {
            let pr = match l.pr {
                Some(n) => format!("  (PR #{n})"),
                None => String::new(),
            };
            out.push_str(&format!(
                "  L{}  `{}`  {}  {}{}\n",
                l.line_no, l.short_sha, l.date, l.subject, pr
            ));
        }
        out
    }
}

/// Look up the provenance of `path` line(s) `[start, end]` (1-based,
/// inclusive) at `HEAD`. Runs `git blame` for the range, reports each
/// line's introducing commit (short SHA, subject, date), AND associates a
/// PR by parsing the commit subject for GitHub's squash/merge convention.
/// When the subject names no PR, the commit is reported alone — NEVER a
/// fabricated PR. Read-only.
pub fn analyze_provenance(
    workspace: &Path,
    path: &str,
    start: usize,
    end: usize,
    detect_moves: bool,
) -> Result<ProvenanceReport> {
    let blamed = git::blame_lines(workspace, "HEAD", path, Some((start, end)), detect_moves)?;
    let lines = blamed
        .into_iter()
        .map(|b| ProvenanceLine {
            line_no: b.line_no,
            short_sha: short(&b.sha),
            pr: git::pr_number_from_subject(&b.subject),
            subject: b.subject,
            date: b.date,
        })
        .collect();
    Ok(ProvenanceReport {
        path: path.to_string(),
        lines,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    fn run(path: &Path, args: &[&str]) {
        let st = Command::new("git").args(args).current_dir(path).status().unwrap();
        assert!(st.success(), "git {args:?} failed");
    }

    fn repo() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_path_buf();
        run(&path, &["init", "-q", "-b", "main"]);
        run(&path, &["config", "user.email", "t@example.com"]);
        run(&path, &["config", "user.name", "t"]);
        std::fs::write(path.join("README.md"), "readme\n").unwrap();
        run(&path, &["add", "README.md"]);
        run(&path, &["commit", "-q", "-m", "initial"]);
        (dir, path)
    }

    fn head(path: &Path) -> String {
        git::rev_parse(path, "HEAD").unwrap()
    }

    #[test]
    fn lines_to_regions_collapses_contiguous_runs() {
        let set: BTreeSet<usize> = [1usize, 2, 3, 7, 9, 10].into_iter().collect();
        let regions = lines_to_regions(&set);
        assert_eq!(
            regions,
            vec![
                LineRegion { start: 1, end: 3 },
                LineRegion { start: 7, end: 7 },
                LineRegion { start: 9, end: 10 },
            ]
        );
        assert_eq!(lines_to_regions(&BTreeSet::new()), vec![]);
    }

    /// 6.1 — a file untouched since the target is reported fully surviving
    /// via the pre-filter (the pre-filter sees no later touch). We also
    /// assert the file ends up in the pre-filter branch, not blame.
    #[test]
    fn untouched_file_fully_surviving_via_prefilter() {
        let (_dir, path) = repo();
        std::fs::write(path.join("stable.rs"), "fn stable() {}\n").unwrap();
        run(&path, &["add", "stable.rs"]);
        run(&path, &["commit", "-q", "-m", "add stable"]);
        let target = head(&path);
        // A later commit touches a DIFFERENT file only.
        std::fs::write(path.join("other.rs"), "fn other() {}\n").unwrap();
        run(&path, &["add", "other.rs"]);
        run(&path, &["commit", "-q", "-m", "add other"]);

        let report = analyze_survival(
            &path,
            "main",
            "origin",
            &SurvivalTarget::Commit { sha: target },
            false,
        )
        .unwrap();
        let stable = report
            .files
            .iter()
            .find(|f| f.path == "stable.rs")
            .expect("stable.rs in report");
        assert_eq!(
            stable.survival,
            FileSurvival::FullyViaPreFilter,
            "untouched file → pre-filter, no blame"
        );
        assert_eq!(report.fully_surviving_files(), 1);
    }

    /// 6.2 — a later-modified file: the target's still-attributed lines are
    /// surviving; overwritten lines are not.
    #[test]
    fn later_modified_file_resolved_line_by_line() {
        let (_dir, path) = repo();
        // Target commit writes 3 lines.
        std::fs::write(path.join("f.rs"), "keep1\noverwrite\nkeep2\n").unwrap();
        run(&path, &["add", "f.rs"]);
        run(&path, &["commit", "-q", "-m", "target writes f"]);
        let target = head(&path);
        // A later commit overwrites line 2 only.
        std::fs::write(path.join("f.rs"), "keep1\nNEW_LINE\nkeep2\n").unwrap();
        run(&path, &["add", "f.rs"]);
        run(&path, &["commit", "-q", "-m", "later rewrites line 2"]);

        let report = analyze_survival(
            &path,
            "main",
            "origin",
            &SurvivalTarget::Commit { sha: target },
            false,
        )
        .unwrap();
        let f = report.files.iter().find(|f| f.path == "f.rs").unwrap();
        match &f.survival {
            FileSurvival::Partial { regions, surviving } => {
                assert_eq!(*surviving, 2, "lines 1 and 3 survive, line 2 overwritten");
                assert_eq!(
                    regions,
                    &vec![
                        LineRegion { start: 1, end: 1 },
                        LineRegion { start: 3, end: 3 },
                    ],
                    "surviving regions are lines 1 and 3"
                );
            }
            other => panic!("expected Partial, got {other:?}"),
        }
    }

    /// 6.3 — the report states the verbatim boundary AND never reports a
    /// line as surviving unless blame attributes it to the target. Here a
    /// later commit overwrites EVERY target line, so nothing survives — and
    /// the rendered report must NOT claim any surviving line.
    #[test]
    fn report_states_boundary_and_never_overreports() {
        let (_dir, path) = repo();
        std::fs::write(path.join("g.rs"), "a\nb\n").unwrap();
        run(&path, &["add", "g.rs"]);
        run(&path, &["commit", "-q", "-m", "target writes g"]);
        let target = head(&path);
        // Rewrite ALL lines.
        std::fs::write(path.join("g.rs"), "x\ny\nz\n").unwrap();
        run(&path, &["add", "g.rs"]);
        run(&path, &["commit", "-q", "-m", "rewrite all"]);

        let report = analyze_survival(
            &path,
            "main",
            "origin",
            &SurvivalTarget::Commit { sha: target },
            false,
        )
        .unwrap();
        let g = report.files.iter().find(|f| f.path == "g.rs").unwrap();
        assert_eq!(g.survival, FileSurvival::None, "all lines overwritten");
        assert_eq!(report.total_surviving_lines(), 0);

        let rendered = report.render();
        assert!(
            rendered.contains("VERBATIM survival"),
            "report must state the verbatim boundary: {rendered}"
        );
        assert!(
            rendered.contains("UNDER-reports") && rendered.contains("never over-reports"),
            "boundary must spell out under/over reporting: {rendered}"
        );
        assert!(
            rendered.to_lowercase().contains("commit "),
            "report names the target: {rendered}"
        );
    }

    /// 6.4 — provenance: a current line maps to its introducing commit; the
    /// PR is named when discoverable and omitted (commit-only) when not.
    #[test]
    fn provenance_maps_line_to_commit_and_pr_when_discoverable() {
        let (_dir, path) = repo();
        // Line introduced by a squash-style commit naming PR #42.
        std::fs::write(path.join("h.rs"), "with_pr\n").unwrap();
        run(&path, &["add", "h.rs"]);
        run(&path, &["commit", "-q", "-m", "add with_pr (#42)"]);
        // A second line by a commit with NO PR marker.
        std::fs::write(path.join("h.rs"), "with_pr\nno_pr\n").unwrap();
        run(&path, &["add", "h.rs"]);
        run(&path, &["commit", "-q", "-m", "add no_pr line"]);

        let report = analyze_provenance(&path, "h.rs", 1, 2, false).unwrap();
        assert_eq!(report.lines.len(), 2, "{:?}", report.lines);
        assert_eq!(report.lines[0].pr, Some(42), "line 1 traces to PR #42");
        assert_eq!(report.lines[0].subject, "add with_pr (#42)");
        assert!(!report.lines[0].short_sha.is_empty());
        assert_eq!(report.lines[1].pr, None, "line 2 has no PR association");

        let rendered = report.render();
        assert!(rendered.contains("PR #42"), "PR named when found: {rendered}");
        // Line 2 must show its commit but NO fabricated PR.
        assert!(
            !rendered.lines().any(|l| l.contains("no_pr") && l.contains("PR #")),
            "no fabricated PR for the unmarked commit: {rendered}"
        );
    }

    /// 6.5 — both analyses are read-only: HEAD AND the working tree are
    /// unchanged after running them.
    #[test]
    fn analyses_are_read_only() {
        let (_dir, path) = repo();
        std::fs::write(path.join("r.rs"), "alpha\nbeta\n").unwrap();
        run(&path, &["add", "r.rs"]);
        run(&path, &["commit", "-q", "-m", "target"]);
        let target = head(&path);
        std::fs::write(path.join("r.rs"), "alpha\nGAMMA\n").unwrap();
        run(&path, &["add", "r.rs"]);
        run(&path, &["commit", "-q", "-m", "later"]);

        let head_before = head(&path);
        let status_before = git::status_porcelain(&path).unwrap();

        let _ = analyze_survival(
            &path,
            "main",
            "origin",
            &SurvivalTarget::Commit { sha: target },
            false,
        )
        .unwrap();
        let _ = analyze_provenance(&path, "r.rs", 1, 2, false).unwrap();

        assert_eq!(head(&path), head_before, "HEAD must be unchanged");
        assert_eq!(
            git::status_porcelain(&path).unwrap(),
            status_before,
            "working tree must be unchanged"
        );
    }

    #[test]
    fn pr_target_resolves_squash_commit_set() {
        let (_dir, path) = repo();
        std::fs::write(path.join("feat.rs"), "feat\n").unwrap();
        run(&path, &["add", "feat.rs"]);
        run(&path, &["commit", "-q", "-m", "add feat (#13)"]);
        let sha = head(&path);
        // No network ref exists; resolution must succeed off the base-branch
        // squash subject alone.
        let shas = resolve_target_commits(&path, "main", "origin", &SurvivalTarget::Pr { number: 13 })
            .unwrap();
        assert!(shas.contains(&sha), "squash commit in the PR commit-set: {shas:?}");
    }

    #[test]
    fn render_emits_review_focus_for_surviving_files() {
        let (_dir, path) = repo();
        std::fs::write(path.join("live.rs"), "still_here\n").unwrap();
        run(&path, &["add", "live.rs"]);
        run(&path, &["commit", "-q", "-m", "add live"]);
        let target = head(&path);
        // Unrelated later commit so the report is non-trivial.
        std::fs::write(path.join("z.rs"), "z\n").unwrap();
        run(&path, &["add", "z.rs"]);
        run(&path, &["commit", "-q", "-m", "z"]);

        let report = analyze_survival(
            &path,
            "main",
            "origin",
            &SurvivalTarget::Commit { sha: target },
            false,
        )
        .unwrap();
        assert_eq!(report.review_focus_paths(), vec!["live.rs".to_string()]);
        let rendered = report.render();
        assert!(
            rendered.contains("files live.rs"),
            "surviving file is emitted as a review target: {rendered}"
        );
    }
}
