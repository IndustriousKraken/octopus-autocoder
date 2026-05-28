## 1. `build.rs`

- [x] 1.1 Create `autocoder/build.rs` at the crate root:
  ```rust
  fn main() {
      let describe = std::process::Command::new("git")
          .args(["describe", "--tags", "--always", "--dirty"])
          .current_dir(env!("CARGO_MANIFEST_DIR"))
          .output()
          .ok()
          .filter(|o| o.status.success())
          .and_then(|o| String::from_utf8(o.stdout).ok())
          .map(|s| s.trim().to_string())
          .filter(|s| !s.is_empty())
          .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
      println!("cargo:rustc-env=AUTOCODER_VERSION={describe}");

      // Re-run when HEAD, the index, or any tag ref changes so dev builds
      // reflect the working commit. Skipping this means cargo caches
      // the AUTOCODER_VERSION across commits.
      println!("cargo:rerun-if-changed=.git/HEAD");
      println!("cargo:rerun-if-changed=.git/index");
      println!("cargo:rerun-if-changed=.git/refs/tags");
  }
  ```
- [x] 1.2 Cargo auto-detects `build.rs` at crate root; no `Cargo.toml` edit needed. Verify on this version of cargo by running `cargo build` once AND checking that `env!("AUTOCODER_VERSION")` resolves.
- [x] 1.3 The fallback chain: git command not found OR exits non-zero OR returns empty → use `env!("CARGO_PKG_VERSION")` verbatim. The fallback NEVER produces an empty string AND NEVER fails the build.
- [x] 1.4 Test the fallback manually: rename `.git/` to `.git.bak/` temporarily, run `cargo build --release`, confirm `--version` returns the Cargo.toml version verbatim. Restore `.git/`.

## 2. Replace `env!("CARGO_PKG_VERSION")` references

- [x] 2.1 Sweep the codebase:
  ```bash
  grep -rn 'CARGO_PKG_VERSION' autocoder/src/
  ```
- [x] 2.2 For each hit, replace with `env!("AUTOCODER_VERSION")`. Likely sites: the `🆙` startup notification in the daemon's run-loop bring-up; the clap derive macro for the top-level `Cli` struct; any log line that prints version at startup.
- [x] 2.3 Tests covering version output:
  - `autocoder --version` returns a non-empty string.
  - The string matches `env!("AUTOCODER_VERSION")` exactly.
  - In a CI environment where `.git/` is present, the string includes a hash suffix OR matches a tag — never literally `0.1.0` (the dev fallback).

## 3. Clap `--version` override

- [x] 3.1 Locate the top-level `Cli` struct (likely `autocoder/src/cli/mod.rs`).
- [x] 3.2 Update the `#[command(...)]` attribute:
  ```rust
  #[derive(Parser)]
  #[command(
      name = "autocoder",
      version = env!("AUTOCODER_VERSION"),
      about = "..."
  )]
  pub struct Cli { ... }
  ```
- [x] 3.3 Test: `cargo run -- --version` outputs the describe-derived string, not the Cargo.toml version.

## 4. Startup notification update

- [x] 4.1 In whatever module hosts the `🆙` post (per `a04`'s implementation), change:
  ```rust
  let version = env!("CARGO_PKG_VERSION");
  ```
  to:
  ```rust
  let version = env!("AUTOCODER_VERSION");
  ```
- [x] 4.2 The `format!("🆙 autocoder v{} started — {} repository(ies) configured", version, count)` line is unchanged. Only the source of `version` changes.
- [x] 4.3 Test (using `MockChatOpsBackend` per `a04`'s test pattern):
  - Boot the daemon's bring-up function.
  - Assert the `post_notification` message text contains the `env!("AUTOCODER_VERSION")` value.
  - Assert the message follows the format `🆙 autocoder v<version> started — <N> repository(ies) configured`.

## 5. Docs

- [x] 5.1 In `docs/DEPLOYMENT.md`, add a "Version-string format" section under the existing "Upgrading" discussion:
  - Binary-release operators (using `update.sh`) see clean `vX.Y.Z` strings because the release workflow builds at tagged commits.
  - Source-build operators see `vX.Y.Z-N-gSHA` strings where N is the count of commits past the last tag AND SHA is the abbreviated commit hash. A `-dirty` suffix appears when the build includes uncommitted local changes.
  - Cargo.toml's `version =` field is the "base version operators manually bump at semver-meaningful releases." It's not bumped per commit; `git describe` provides the delta info automatically.
- [x] 5.2 In `docs/CHATOPS.md`, update the `🆙` startup-notification example. Pre-spec:
  ```
  🆙 autocoder v0.1.0 started — 8 repository(ies) configured
  ```
  Post-spec, two example forms:
  ```
  🆙 autocoder v1.1.1 started — 8 repository(ies) configured
  🆙 autocoder v1.1.1-23-g4abc123 started — 8 repository(ies) configured
  ```
  with a one-liner explaining when each form appears.

## 6. Spec deltas

- [x] 6.1 `openspec/changes/a20-version-string-from-git-describe/specs/orchestrator-cli/spec.md` MODIFIES `Daemon emits a startup version notification on every successful boot` (preserves all 4 existing scenarios) AND ADDs `Binary version string is derived from \`git describe\` at build time`.
- [x] 6.2 `openspec/changes/a20-version-string-from-git-describe/specs/project-documentation/spec.md` ADDs `DEPLOYMENT.md AND CHATOPS.md explain the version-string format AND the source-vs-binary distinction`.

## 7. Verification

- [x] 7.1 `cargo test` passes (new + existing).
- [x] 7.2 `openspec validate a20-version-string-from-git-describe --strict` passes.
- [x] 7.3 `cargo clippy --all-targets --all-features -- -D warnings` produces no new warnings.
- [x] 7.4 Manual verification: build the daemon from the current repo; run `./target/release/autocoder --version`; assert the output is the `git describe` form, NOT `0.1.0`.
