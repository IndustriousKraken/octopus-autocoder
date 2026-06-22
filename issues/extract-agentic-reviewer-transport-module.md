# Extract the agentic reviewer transport into its own module

## Problem

`autocoder/src/code_reviewer.rs` (~5,400 lines) contains a self-contained
"agentic reviewer transport" block sitting inside the larger reviewer file with no
module boundary. It is a cohesive unit that belongs behind its own boundary. This
is a maintainability signal, not a defect.

## Desired end state

The agentic reviewer transport lives in its own module (e.g.
`code_reviewer/agentic.rs` or `code_reviewer_agentic.rs`), re-exported so callers
outside the reviewer keep compiling. Reviewer output (the PR-body `## Code Review`
Markdown, verdicts, concerns) is identical.

## Tasks

- [ ] Move the agentic reviewer transport block into its own module: the
  role/tooling consts and `agentic_review_allowed_tools`, the `Raw*` review
  submission types, `payload_to_review_result`, `render_review_submission_markdown`,
  `render_agentic_review_prompt`, the `ReviewSessionRunner` trait,
  `CliReviewSessionRunner`, the `run_agentic_review_*` orchestration, and
  `resolve_reviewer_strategy`. Re-locate via the SYMBOL names — line numbers have
  drifted (this file was recently modified).
- [ ] Re-export the items that callers outside the reviewer reference so existing
  paths keep compiling.
- [ ] Verify: `cargo build` and the existing suite pass; reviewer Markdown,
  verdicts, and concerns are unchanged.

## Constraints (behavior-preserving refactor)

- No observable contract change — reviewer output stays identical. This is
  reorganization, not a feature change. No spec delta.
- Keep public call sites compiling by re-exporting moved items (`pub(crate) use`)
  from their original module path.
- Moved unit tests go to a sibling test module, not a fresh inline
  `#[cfg(test)] mod tests` in the new file.
- Match the surrounding hand-formatting; do NOT run `cargo fmt` (this crate is
  intentionally not rustfmt-clean).
- Do not author or restate any size threshold as a spec requirement — the line
  counts are audit selectors, not contracts.
- Verify against a reliably-green test suite — a behavior-preserving refactor
  checked by a flaky suite proves nothing.
