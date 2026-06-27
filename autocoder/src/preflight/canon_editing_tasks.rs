//! `tasks.md`-content pre-flight: reject a change whose tasks direct an
//! edit to the canonical specs (`openspec/specs/`).
//!
//! A change's `tasks.md` is the implementer's marching orders, and the
//! implementer implements CODE and TESTS only — the change's spec delta lives
//! in its own `specs/<capability>/spec.md` and is folded into the canonical
//! specs by `openspec archive` automatically. When a task instead directs the
//! implementer to apply the delta to `openspec/specs/` directly, the
//! implementer pre-folds the requirement into canon; `openspec archive` then
//! tries to fold the same delta, finds it already present, and aborts on a
//! duplicate-ADD. The change fails every iteration and goes perma-stuck.
//!
//! This module is a sibling of [`crate::preflight::spec_archivability`]: it
//! runs at the same point in the pipeline (before the executor, every change,
//! every iteration) and reuses the same failure plumbing, but it inspects
//! `tasks.md` CONTENT rather than delta headers. The heuristic is mechanical
//! and precision-biased — it pairs a mutation verb with a canonical-specs
//! target — so it stays cheap (no LLM) and reliably catches the observed
//! literal-path case without flagging legitimate code-and-tests work.

use std::path::Path;

/// Mutation verbs that, paired with a canonical-specs target, flag a task.
/// Matched as whole lowercase word tokens (case-insensitive), so prose like
/// "padding" or "addendum" never trips the "add" verb.
const MUTATION_VERBS: &[&str] = &[
    "apply", "add", "copy", "write", "edit", "update", "insert", "append", "paste", "create",
    "populate",
];

/// Scan `<workspace>/openspec/changes/<change_slug>/tasks.md` and return the
/// text of every task that directs an edit to the canonical specs — a mutation
/// verb paired with a canonical-specs target (the path segment
/// `openspec/specs/`, OR the words `canon` / `canonical spec`).
///
/// A reference to the change's OWN delta — a path under
/// `openspec/changes/<slug>/specs/` or a bare `specs/<cap>/spec.md` — is NOT a
/// canonical-specs target (neither contains the `openspec/specs/` segment), so
/// it is not flagged. A read-only mention of canon carries no mutation verb and
/// is not flagged either.
///
/// Returns the collapsed text (id + description) of each offending task, in
/// document order. An empty `Vec` means nothing to flag. A missing or unreadable
/// `tasks.md` returns empty — structural problems are `openspec validate`'s job,
/// and a code-only change without canon edits must proceed.
pub fn check_tasks_edit_canon(workspace_root: &Path, change_slug: &str) -> Vec<String> {
    let tasks_md = workspace_root
        .join("openspec/changes")
        .join(change_slug)
        .join("tasks.md");
    let body = match std::fs::read_to_string(&tasks_md) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            tracing::warn!(
                change = %change_slug,
                "check_tasks_edit_canon: cannot read {}: {e}; treating as no canon-editing tasks",
                tasks_md.display()
            );
            return Vec::new();
        }
    };
    parse_tasks(&body)
        .into_iter()
        .filter(|task| directs_canon_edit(task))
        .collect()
}

/// Split a `tasks.md` body into the collapsed text of each checkbox task.
/// A task starts at a `- [ ]` / `- [x]` line; wrapped continuation lines (the
/// immediately following non-blank, non-heading, non-checkbox lines) are joined
/// onto it with single spaces so a verb and a target split across a line wrap
/// are still seen together. A blank line or a heading ends the current task.
fn parse_tasks(body: &str) -> Vec<String> {
    let mut tasks: Vec<String> = Vec::new();
    let mut current: Option<String> = None;
    for line in body.lines() {
        let trimmed = line.trim();
        if let Some(rest) = strip_checkbox(trimmed) {
            if let Some(task) = current.take() {
                tasks.push(task);
            }
            current = Some(rest.to_string());
        } else if trimmed.is_empty() || trimmed.starts_with('#') {
            if let Some(task) = current.take() {
                tasks.push(task);
            }
        } else if let Some(task) = current.as_mut() {
            task.push(' ');
            task.push_str(trimmed);
        }
    }
    if let Some(task) = current.take() {
        tasks.push(task);
    }
    tasks
}

/// Strip a leading markdown checkbox (`- [ ]`, `- [x]`, `- [X]`) from an
/// already-trimmed line, returning the remainder (the task text, id included).
/// `None` when the line is not a checkbox item.
fn strip_checkbox(trimmed: &str) -> Option<&str> {
    let rest = trimmed.strip_prefix("- [")?;
    let mut chars = rest.chars();
    let status = chars.next()?;
    if !matches!(status, ' ' | 'x' | 'X') {
        return None;
    }
    let after_status = chars.as_str();
    Some(after_status.strip_prefix(']')?.trim_start())
}

/// True when a single task pairs a mutation verb with a canonical-specs target.
fn directs_canon_edit(task: &str) -> bool {
    let lower = task.to_lowercase();
    let has_canon_target = lower.contains("openspec/specs/")
        || lower.contains("canonical spec")
        || tokens(&lower).any(|t| t == "canon");
    if !has_canon_target {
        return false;
    }
    tokens(&lower).any(|t| MUTATION_VERBS.contains(&t))
}

/// Lowercase word tokens of `lower` (already lowercased), splitting on every
/// non-alphanumeric character so path separators, backticks, and punctuation
/// don't fuse words.
fn tokens(lower: &str) -> impl Iterator<Item = &str> {
    lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_tasks(workspace: &Path, change: &str, body: &str) {
        let dir = workspace.join("openspec/changes").join(change);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("tasks.md"), body).unwrap();
    }

    #[test]
    fn apply_block_to_openspec_specs_is_flagged() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        write_tasks(
            ws,
            "c1",
            "## 1. Spec update\n- [ ] 1.1 Apply the ADDED Requirements block from specs/scheduled-payments/spec.md to openspec/specs/scheduled-payments/spec.md\n",
        );
        let flagged = check_tasks_edit_canon(ws, "c1");
        assert_eq!(flagged.len(), 1, "got {flagged:#?}");
        assert!(flagged[0].contains("openspec/specs/scheduled-payments/spec.md"));
    }

    #[test]
    fn each_mutation_verb_paired_with_canon_path_flags() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        for verb in MUTATION_VERBS {
            write_tasks(
                ws,
                "c",
                &format!("- [ ] 1.1 {verb} the requirement into openspec/specs/cap/spec.md\n"),
            );
            let flagged = check_tasks_edit_canon(ws, "c");
            assert_eq!(flagged.len(), 1, "verb `{verb}` should flag; got {flagged:#?}");
        }
    }

    #[test]
    fn canon_word_target_is_flagged() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        write_tasks(ws, "c1", "- [ ] 1.1 Add the new requirement to canon directly\n");
        assert_eq!(check_tasks_edit_canon(ws, "c1").len(), 1);
    }

    #[test]
    fn canonical_spec_words_target_is_flagged() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        write_tasks(
            ws,
            "c1",
            "- [ ] 1.1 Update the canonical spec for scheduled-payments\n",
        );
        assert_eq!(check_tasks_edit_canon(ws, "c1").len(), 1);
    }

    #[test]
    fn own_delta_path_is_not_flagged() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        // The change's own delta lives under openspec/changes/<slug>/specs/ —
        // editing it is the legitimate spec-delta authoring, not a canon edit.
        write_tasks(
            ws,
            "c1",
            "- [ ] 1.1 Add a scenario to openspec/changes/c1/specs/cap/spec.md\n",
        );
        assert!(check_tasks_edit_canon(ws, "c1").is_empty());
    }

    #[test]
    fn bare_change_delta_path_is_not_flagged() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        // A bare `specs/<cap>/spec.md` (the change's own delta in shorthand) is
        // not the canonical `openspec/specs/` path.
        write_tasks(
            ws,
            "c1",
            "- [ ] 1.1 Write the MODIFIED block in specs/cap/spec.md for this change\n",
        );
        assert!(check_tasks_edit_canon(ws, "c1").is_empty());
    }

    #[test]
    fn read_only_canon_reference_is_not_flagged() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        write_tasks(
            ws,
            "c1",
            "- [ ] 1.1 Ensure the change matches the existing contract in openspec/specs/cap/spec.md\n",
        );
        // No mutation verb directs a write — "ensure"/"matches" are read-only.
        assert!(check_tasks_edit_canon(ws, "c1").is_empty());
    }

    #[test]
    fn clean_code_and_tests_tasks_are_not_flagged() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        write_tasks(
            ws,
            "c1",
            "## 1. Add validation\n- [ ] 1.1 In `src/handlers/upload.rs::receive_file`, reject `..` paths\n- [ ] 1.2 Add unit test `receive_file_rejects_path_traversal`\n",
        );
        assert!(check_tasks_edit_canon(ws, "c1").is_empty());
    }

    #[test]
    fn verb_as_substring_of_another_word_does_not_flag() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        // "padding" contains "add", "addendum" contains "add" — token equality
        // means neither trips the verb requirement.
        write_tasks(
            ws,
            "c1",
            "- [ ] 1.1 Document the padding addendum referencing openspec/specs/cap/spec.md\n",
        );
        assert!(check_tasks_edit_canon(ws, "c1").is_empty());
    }

    #[test]
    fn wrapped_task_pairs_verb_and_target_across_lines() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        // The verb is on the checkbox line; the canonical path is on the wrapped
        // continuation line. The pairing must still be seen.
        write_tasks(
            ws,
            "c1",
            "- [ ] 1.1 Apply the ADDED Requirements block\n  to openspec/specs/cap/spec.md so canon is updated\n",
        );
        assert_eq!(check_tasks_edit_canon(ws, "c1").len(), 1);
    }

    #[test]
    fn missing_tasks_md_returns_empty() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        std::fs::create_dir_all(ws.join("openspec/changes/c1")).unwrap();
        assert!(check_tasks_edit_canon(ws, "c1").is_empty());
    }

    /// Behaviour check on the audit prompts (task 3.4): the example `tasks.md`
    /// checklists the spec-writing audits emit must carry NO canon-editing
    /// task. Derived from the produced-artifact shape (the prompts' embedded
    /// `- [ ]` example lines), not from asserting a prompt substring.
    #[test]
    fn audit_prompt_example_tasks_carry_no_canon_edit() {
        const SECURITY_AUDIT: &str = include_str!("../../../prompts/security-bug-audit.md");
        const MISSING_TESTS_AUDIT: &str = include_str!("../../../prompts/missing-tests-audit.md");
        for (name, prompt) in [
            ("security-bug-audit", SECURITY_AUDIT),
            ("missing-tests-audit", MISSING_TESTS_AUDIT),
        ] {
            let example_tasks: Vec<String> =
                parse_tasks(prompt).into_iter().filter(|t| directs_canon_edit(t)).collect();
            assert!(
                example_tasks.is_empty(),
                "{name} example tasks must not direct a canon edit; got {example_tasks:#?}"
            );
        }
    }
}
