# Implementation tasks

## 1. Extract shared lane utilities (behavior-preserving)

- [x] 1.1 Lift the leaf primitives the lanes share into a stateless shared-utility module: busy-marker acquire/release, PR opening, archive-with-postcondition, chatops notify, queue-state I/O, AND workspace handling. Behavior for the existing changes walker is unchanged.
- [x] 1.2 Confirm each primitive has a single definition composed by callers, not duplicated per lane.

## 2. Issues lane artifact + lifecycle

- [x] 2.1 Load an `issues/<slug>/` unit: require `issue.md` + `tasks.md`; reject a unit that contains a `specs/` directory as malformed (an issue carries no delta).
- [x] 2.2 Gate the lane behind a `features.issues` flag, off by default.
- [x] 2.3 On completion, move `issues/<slug>/` to `issues/archive/` (mirroring `changes/archive/`) without modifying any canonical spec.

## 3. Issues walker

- [x] 3.1 Add an issues walker with its own control flow and its own state file, composing the shared utilities from §1. Keep lane-specific behavior in the walker, not in shared branching.
- [x] 3.2 Ensure each walker reads and writes only its own lane's state (fault isolation between lanes).

## 4. Lane precedence

- [x] 4.1 Extend the polling iteration's unit selection to pick the highest-precedence ready unit in the order issues > changes > audits; alphabetical within a lane. Issue-precedence is strict.

## 5. Issue-flavored implementer prompt (`executor`)

- [x] 5.1 Add an issue-flavored implementer prompt: fix to match the existing spec; do not write a spec change; kick a behavior-change fix back to the changes lane. Load it through the uniform PromptLoader and declare its override field per the nested naming convention.
- [x] 5.2 Route an issue run through the issue-flavored prompt; verify acceptance against the existing canon.

## 6. Tests

- [x] 6.1 An `issues/<slug>/` with a `specs/` directory is rejected as malformed.
- [x] 6.2 With `features.issues` on, a ready issue is selected before a ready change, and a change before an audit; two ready issues select alphabetically.
- [x] 6.3 With `features.issues` unset, `issues/<slug>/` directories are not worked.
- [x] 6.4 The changes walker and the issues walker read/write separate state files; the shared primitives have one definition each.
- [x] 6.5 Completion moves the issue to `issues/archive/` and modifies no canonical spec.
- [x] 6.6 An issue run uses the issue-flavored prompt; a fix requiring behavior change is reported back to the changes lane without altering any spec.

## 7. Acceptance gate

- [x] 7.1 `cargo test` passes for the autocoder crate.
- [x] 7.2 `cargo clippy --all-targets -- -D warnings` is clean.
- [x] 7.3 `openspec validate a009-issues-lane-curated --strict` passes.
