## Why

Today autocoder is `git clone + cargo build --release` only. There are no tags, no GitHub Releases, no pre-built binaries. Every new operator who wants to try it has to install the full Rust toolchain and wait through a release build, which gates adoption at a step that has nothing to do with autocoder itself. The companion `install-script-and-wizard` change cannot work without per-tag pre-built binaries to download — this change supplies them.

The shape of "tagged binary releases driven by GitHub Actions" is well-trodden ground in the Rust ecosystem (`ripgrep`, `bat`, `fd`, etc.) and we should follow the same conventions: matrix-build across the architectures we care about, gate on green tests, attach checksums, name assets predictably so a shell script can download them by pattern.

## What Changes

- **NEW**: `.github/workflows/release.yml` — triggered on tag push matching `v*`. Stages:
  1. **Test gate** (single job, runs first): `cargo test --release` on `ubuntu-latest`. If this fails, no binaries are built and the release is not published.
  2. **Build matrix** (runs after the test gate passes): one job per target triple:
     - `x86_64-unknown-linux-gnu` (ubuntu-latest runner; native build).
     - `aarch64-unknown-linux-gnu` (ubuntu-latest runner with `cross`).
     - `aarch64-apple-darwin` (macos-latest runner; native build on Apple Silicon).
     Each job: `cargo build --release --target <triple>`, `strip` the resulting binary, compute SHA-256, upload `autocoder-<version>-<triple>` and `autocoder-<version>-<triple>.sha256` as job artifacts.
  3. **Publish job** (runs after the matrix completes): downloads all artifacts, creates a GitHub Release for the tag using `softprops/action-gh-release` (or equivalent), uploads all binaries + checksum files as release assets, marks the release as `prerelease: true` IFF the tag matches `v*-*` (the SemVer dash-suffix convention for pre-release versions).
- **Asset naming convention** (pinned in this spec so the install script can rely on it): `autocoder-<full-version-tag>-<rust-target-triple>` for the binary, with `.sha256` appended for the checksum file. Examples:
  - `autocoder-v1.0.0-x86_64-unknown-linux-gnu`
  - `autocoder-v1.0.0-x86_64-unknown-linux-gnu.sha256`
  - `autocoder-v1.2.3-rc1-aarch64-apple-darwin`
- **NEW**: `RELEASING.md` (or a section in `CONTRIBUTING.md` if one exists later) — short operator-facing doc explaining the release procedure for a maintainer: bump version in `Cargo.toml`, `git tag vX.Y.Z`, `git push --tags`, watch the workflow, edit release notes after publish.
- **ADDED requirement** under `project-documentation`: "Tagged releases produce architecture-specific binaries on GitHub Releases" — pins the trigger condition, the build-test gate, the asset naming convention, and the pre-release detection rule so future installer code can rely on the contract.

## Impact

- Affected specs: `project-documentation` (one ADDED requirement establishing the release-pipeline contract).
- Affected code: new `.github/workflows/release.yml`. No changes to the Rust source tree.
- New repo files: `.github/workflows/release.yml`, `RELEASING.md` (or equivalent).
- Operator-visible behavior: maintainers gain the ability to publish releases by pushing a tag. The first release will require manually pushing a `v0.1.0` tag (or whatever version is appropriate for the current state). Pre-release tags (`v0.1.0-rc1`, etc.) work the same way and are auto-marked as pre-release on the GitHub Release page.
- Cost: GitHub Actions minutes per release (one ubuntu test job + three build jobs ≈ 15–25 minutes total per tag push). Free tier accommodates many releases.
- Security: binaries are built in GitHub-hosted runners from the tagged commit. Anyone can verify by inspecting the workflow log + downloading the release and re-building from source. SHA-256 sums attached to each binary let the install script (and operators) verify what they downloaded matches what the workflow produced.
- Breaking: no existing functionality changes. Source-build path remains supported.
- Acceptance: `openspec validate release-pipeline-github-actions --strict` passes. The workflow file is linted on every PR by an `actionlint` step that runs as a GitHub Action (`rhysd/actionlint@v1`) — no maintainer-side install needed. A `workflow_dispatch:` trigger with a `dry_run` input lets a maintainer exercise the matrix from the Actions tab without creating a throwaway tag (when `dry_run: true`, test + build run but publish is skipped). The first real `v*` tag push is the integration test for the publish stage; if it fails, the maintainer addresses the failure via a follow-up change, same as any other CI failure. The spec does NOT mandate a pre-merge smoke-test ritual — autocoder cannot push tags from its sandbox, so any "verify by pushing a tag" task would be an unimplementable buck-pass.
