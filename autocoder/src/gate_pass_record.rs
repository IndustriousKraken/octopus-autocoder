//! Sequence-scoped gate-pass record (iteration-sequence-gates-once).
//!
//! When every enabled pre-executor gate passes for a change in one pickup, the
//! daemon records a content hash of the change directory's gate inputs at
//! `<state>/gate-pass/<workspace-basename>/<change>.json`. A later CONTINUATION
//! pickup (its iteration-pending marker is present) whose recomputed hash equals
//! the record does NOT re-spawn the gate sessions — the sequence's recorded
//! verdicts stand, rendered in the ledger as carried forward.
//!
//! Any doubt — a missing/unreadable record, a hash mismatch, or any I/O error
//! computing the hash — surfaces as "no usable record", so the caller runs the
//! gates in full. The skip fails toward RUNNING, never toward passing.
//!
//! **State-dir, not workspace.** The record must never ride a commit AND must
//! survive `git clean` / a re-clone, mirroring the iteration-pending marker
//! ([`crate::iteration_pending`]). Workspace-local bookkeeping is vulnerable to
//! both.

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use std::path::Path;

use crate::paths::DaemonPaths;

/// Daemon bookkeeping files that live inside a change's directory and are
/// EXCLUDED from the gate-inputs hash — they are not gate inputs, and the
/// daemon writes/removes them across the sequence (hashing them would spuriously
/// re-gate). The exact list is fixed by design.md.
const EXCLUDED_MARKERS: &[&str] = &[
    ".in-progress",
    ".question.json",
    ".answer.json",
    ".needs-spec-revision.json",
    ".perma-stuck.json",
    ".priority.json",
    ".ignore-for-queue.json",
];

/// On-disk shape of the gate-pass record: the gate-inputs hash the pass recorded
/// AND when it recorded it. Nothing more — the actual per-gate verdicts are read
/// from the per-change gate ledger persisted under `.git/` at the same time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatePassRecord {
    pub inputs_hash: String,
    pub recorded_at: String,
}

/// Write/replace the record atomically (tempfile + rename). Any prior record for
/// the change is superseded. `inputs_hash` is the value from
/// [`compute_inputs_hash`] for the change's directory at record time.
pub fn write_record(
    paths: &DaemonPaths,
    workspace_basename: &str,
    change: &str,
    inputs_hash: &str,
) -> Result<()> {
    let dir = paths.gate_pass_basename_dir(workspace_basename);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating gate-pass dir {}", dir.display()))?;
    let path = paths.gate_pass_path(workspace_basename, change);
    let record = GatePassRecord {
        inputs_hash: inputs_hash.to_string(),
        recorded_at: chrono::Utc::now().to_rfc3339(),
    };
    let tmp = tempfile::NamedTempFile::new_in(&dir)
        .with_context(|| format!("creating gate-pass tempfile in {}", dir.display()))?;
    serde_json::to_writer_pretty(&tmp, &record)
        .with_context(|| format!("serializing gate-pass record for {}", path.display()))?;
    tmp.persist(&path)
        .map_err(|e| anyhow!("atomically persisting {}: {e}", path.display()))?;
    Ok(())
}

/// Idempotent removal of the record. A missing file is success. Called wherever
/// the iteration-pending marker is dropped (the sequence terminated).
pub fn remove_record(
    paths: &DaemonPaths,
    workspace_basename: &str,
    change: &str,
) -> Result<()> {
    let path = paths.gate_pass_path(workspace_basename, change);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("removing {}", path.display())),
    }
}

/// Read AND parse the record. Returns `None` when the file is absent OR
/// unreadable OR corrupt — every one of those is "no usable record" (the caller
/// re-gates). Deliberately collapses all failure modes to `None` so a doubt can
/// never masquerade as a match.
pub fn read_record(
    paths: &DaemonPaths,
    workspace_basename: &str,
    change: &str,
) -> Option<GatePassRecord> {
    let path = paths.gate_pass_path(workspace_basename, change);
    let raw = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// True IFF a record exists for `(workspace_basename, change)` AND the hash
/// recomputed over `change_dir` equals the recorded hash. Any doubt — no record,
/// unreadable record, or a hashing I/O error — returns `false`: the skip fails
/// toward RUNNING, never toward passing.
pub fn hash_matches(
    paths: &DaemonPaths,
    workspace_basename: &str,
    change: &str,
    change_dir: &Path,
) -> bool {
    let Some(record) = read_record(paths, workspace_basename, change) else {
        return false;
    };
    match compute_inputs_hash(change_dir) {
        Ok(current) => current == record.inputs_hash,
        Err(e) => {
            tracing::warn!(
                change = %change,
                change_dir = %change_dir.display(),
                "gate-pass: could not recompute gate-inputs hash; treating as no usable record (re-gating): {e:#}"
            );
            false
        }
    }
}

/// SHA-256 over the sorted list of `(relative path, file bytes)` for every
/// regular file under `change_dir`, excluding the daemon marker files in
/// [`EXCLUDED_MARKERS`]. Deterministic: entries are sorted by relative path AND
/// each contributes a length-prefixed, NUL-separated `(path, bytes)` pair so no
/// two distinct trees can collide by concatenation. Any I/O error (unreadable
/// dir/file) returns `Err` — the caller treats that as "no usable record".
pub fn compute_inputs_hash(change_dir: &Path) -> Result<String> {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    collect_files(change_dir, change_dir, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
    for (rel, bytes) in &files {
        ctx.update(rel.as_bytes());
        ctx.update(&[0]);
        ctx.update(&(bytes.len() as u64).to_le_bytes());
        ctx.update(bytes);
    }
    let digest = ctx.finish();
    let mut hex = String::with_capacity(digest.as_ref().len() * 2);
    for byte in digest.as_ref() {
        // Infallible: writing to a String never errors.
        let _ = write!(hex, "{byte:02x}");
    }
    Ok(hex)
}

/// Recursively collect `(relative-to-root path, bytes)` for every regular file
/// under `dir`, skipping [`EXCLUDED_MARKERS`] by file name. Symlinks are not
/// regular files (their `file_type` reports neither dir nor file) so they are
/// skipped — "every regular file" per the spec.
fn collect_files(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) -> Result<()> {
    let entries =
        std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("reading a dir entry under {}", dir.display()))?;
        let file_type = entry
            .file_type()
            .with_context(|| format!("statting {}", entry.path().display()))?;
        let path = entry.path();
        if file_type.is_dir() {
            collect_files(root, &path, out)?;
        } else if file_type.is_file() {
            if entry
                .file_name()
                .to_str()
                .map(|n| EXCLUDED_MARKERS.contains(&n))
                .unwrap_or(false)
            {
                continue;
            }
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let bytes =
                std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
            out.push((rel, bytes));
        }
        // Anything else (symlink, socket, fifo) is not a regular file: skip.
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::test_daemon_paths;
    use tempfile::TempDir;

    fn write(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn hash_is_stable_and_sensitive_to_content() {
        let dir = TempDir::new().unwrap();
        let cd = dir.path().join("change");
        write(&cd.join("proposal.md"), "## Why\nx\n");
        write(&cd.join("specs/cap/spec.md"), "## ADDED\n### Requirement: A\n");
        let h1 = compute_inputs_hash(&cd).unwrap();
        // Recompute — identical bytes → identical hash.
        assert_eq!(h1, compute_inputs_hash(&cd).unwrap());
        // Edit any file → different hash.
        write(&cd.join("specs/cap/spec.md"), "## ADDED\n### Requirement: B\n");
        assert_ne!(h1, compute_inputs_hash(&cd).unwrap());
    }

    #[test]
    fn excluded_markers_do_not_affect_the_hash() {
        let dir = TempDir::new().unwrap();
        let cd = dir.path().join("change");
        write(&cd.join("proposal.md"), "## Why\nx\n");
        let before = compute_inputs_hash(&cd).unwrap();
        // Dropping any excluded daemon marker into the dir must NOT change it.
        for m in EXCLUDED_MARKERS {
            write(&cd.join(m), "daemon bookkeeping");
        }
        assert_eq!(
            before,
            compute_inputs_hash(&cd).unwrap(),
            "excluded daemon markers must not participate in the gate-inputs hash"
        );
    }

    #[test]
    fn hash_of_missing_dir_is_err() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("nope");
        assert!(compute_inputs_hash(&missing).is_err());
    }

    #[test]
    fn write_read_remove_round_trip() {
        let (_t, paths) = test_daemon_paths();
        assert!(read_record(&paths, "ws", "c1").is_none());
        write_record(&paths, "ws", "c1", "deadbeef").unwrap();
        let got = read_record(&paths, "ws", "c1").expect("record present");
        assert_eq!(got.inputs_hash, "deadbeef");
        remove_record(&paths, "ws", "c1").unwrap();
        assert!(read_record(&paths, "ws", "c1").is_none());
        // Idempotent removal of an absent record.
        remove_record(&paths, "ws", "c1").unwrap();
    }

    #[test]
    fn hash_matches_only_on_exact_inputs() {
        let (_t, paths) = test_daemon_paths();
        let dir = TempDir::new().unwrap();
        let cd = dir.path().join("change");
        write(&cd.join("proposal.md"), "## Why\nx\n");
        let h = compute_inputs_hash(&cd).unwrap();
        write_record(&paths, "ws", "c1", &h).unwrap();
        assert!(hash_matches(&paths, "ws", "c1", &cd), "unchanged inputs match");
        // Edit a file → mismatch.
        write(&cd.join("proposal.md"), "## Why\nedited\n");
        assert!(!hash_matches(&paths, "ws", "c1", &cd), "edited inputs mismatch");
    }

    #[test]
    fn hash_matches_false_when_no_record() {
        let (_t, paths) = test_daemon_paths();
        let dir = TempDir::new().unwrap();
        let cd = dir.path().join("change");
        write(&cd.join("proposal.md"), "## Why\nx\n");
        // No record written → never a match (fail toward running).
        assert!(!hash_matches(&paths, "ws", "c1", &cd));
    }

    #[test]
    fn hash_matches_false_on_unreadable_record() {
        let (_t, paths) = test_daemon_paths();
        let dir = TempDir::new().unwrap();
        let cd = dir.path().join("change");
        write(&cd.join("proposal.md"), "## Why\nx\n");
        // Corrupt record on disk → read_record None → no match.
        let rec_dir = paths.gate_pass_basename_dir("ws");
        std::fs::create_dir_all(&rec_dir).unwrap();
        std::fs::write(paths.gate_pass_path("ws", "c1"), "not json{").unwrap();
        assert!(!hash_matches(&paths, "ws", "c1", &cd));
    }
}
