# Tasks

## 1. Make the issues-lane failure-state write atomic
- [ ] 1.1 In `autocoder/src/lanes/state.rs::record_failure`, replace
  ```rust
  let raw = serde_json::to_string_pretty(&entry)?;
  std::fs::write(&path, raw).with_context(|| format!("writing {}", path.display()))?;
  ```
  with the atomic temp-then-rename pattern used by
  `autocoder/src/failure_state.rs::save_entry`: create
  `tempfile::NamedTempFile::new_in(parent)` in the already-ensured parent
  directory, write the entry with `serde_json::to_writer_pretty(&tmp, &entry)`,
  then `tmp.persist(&path)` mapping the persist error to an
  `anyhow` error with the destination path for context. Keep the existing
  `create_dir_all(parent)` call that precedes it.
- [ ] 1.2 Confirm `tempfile` is already an available dependency (it is used by
  `autocoder/src/failure_state.rs`); no `Cargo.toml` change is expected. If the
  import is missing in `state.rs`, reference it fully-qualified as
  `tempfile::NamedTempFile` as `failure_state.rs` does.

## 2. Test that no torn/partial file is left behind
- [ ] 2.1 Extend the test module in `autocoder/src/lanes/state.rs` with a test
  that calls `record_failure` for a slug and then asserts the per-repo
  issues-state directory contains exactly the single `<slug>.json` entry and no
  leftover temporary file (e.g. no sibling file whose name is not `<slug>.json`),
  confirming the temp file was atomically persisted and cleaned up. Reuse the
  existing `crate::testing::test_daemon_paths` helper and the `repo_dir`/
  `slug_file` layout.
- [ ] 2.2 Keep the existing `record_increments_and_clear_resets` test passing
  (round-trip and counter increment unchanged).

## 3. Verify
- [ ] 3.1 Run `cargo test -p autocoder lanes::state` and confirm the new and
  existing tests pass.
