## 1. Workflow file

- [x] 1.1 Create `.github/workflows/release.yml`. Triggers: `on: push: tags: ['v*']` (the production trigger) AND `on: workflow_dispatch:` with a `dry_run` boolean input (default `true`). The workflow has four logical stages: `lint` (actionlint, runs first), `test` (job with `needs: lint`), `build` (matrix job with `needs: test`), `publish` (job with `needs: build`, skipped entirely when the workflow was launched via workflow_dispatch with `dry_run: true`). The `dry_run` path lets a maintainer exercise the matrix manually from the Actions tab without creating a throwaway tag; the publish stage's `if:` expression checks `github.event_name == 'push' || inputs.dry_run == false` so it only runs on real tag pushes or explicit non-dry-run dispatches.
- [x] 1.2 `lint` job: `runs-on: ubuntu-latest`, no `needs:`. Single step: `uses: rhysd/actionlint@v1`. Catches workflow YAML errors before any test or build runs. This step replaces the previous spec's maintainer-side `actionlint` install — the linter runs as part of CI, not on the maintainer's laptop, so autocoder's sandbox never needs `actionlint` on PATH.
- [x] 1.3 `test` job: `runs-on: ubuntu-latest`, `needs: lint`. Steps: checkout, install stable Rust via `dtolnay/rust-toolchain@stable`, `cd autocoder && cargo test --release --all-features`. Any test failure halts the workflow before any binaries are built.
- [x] 1.4 `build` matrix job: `strategy.matrix.target` covers the three triples. For Linux x86_64: native build on ubuntu-latest. For Linux aarch64: ubuntu-latest with `cross` (via `taiki-e/setup-cross-toolchain-action` or installing `cross` directly). For darwin-aarch64: `runs-on: macos-latest` with native cargo build (Apple Silicon runners are default macos-latest as of 2024+).
- [x] 1.5 Each matrix leg: after `cargo build --release --target <triple>`, run `strip` on the resulting binary (path: `autocoder/target/<triple>/release/autocoder`), copy to a deterministically-named artifact path `autocoder-${{ github.ref_name }}-${{ matrix.target }}`, compute SHA-256 via `shasum -a 256` (or `sha256sum` on Linux), write to `<binary-name>.sha256` in the format `<hex-digest>  <binary-name>` (single space matches what `sha256sum -c` expects). Upload both files as actions artifacts via `actions/upload-artifact@v4`.
- [x] 1.6 `publish` job: `runs-on: ubuntu-latest`, `needs: [test, build]`. The job is gated on `if: github.event_name == 'push' || inputs.dry_run == false` so workflow_dispatch runs with `dry_run: true` do not publish. Steps: download all artifacts via `actions/download-artifact@v4`. Create GitHub Release using `softprops/action-gh-release@v2` with `tag_name: ${{ github.ref_name }}`, `files: <glob to all binaries + .sha256>`, `prerelease: ${{ contains(github.ref_name, '-') }}` (SemVer dash-suffix → pre-release). Set `generate_release_notes: true` so the release body has the auto-generated changelog as a starting point.
- [x] 1.7 `permissions:` block at the top of the workflow: `contents: write` for the publish job (needed by `action-gh-release`). Default for the rest is `read`. The `lint`, `test`, and `build` jobs explicitly set `permissions: { contents: read }` so a workflow-level write permission does not leak into the test/build stages.

## 2. Release procedure doc

- [x] 2.1 Create `RELEASING.md` at the repo root. Short doc (≤ 50 lines). Sections:
  - **Pre-flight**: tests must be green on `main`; `Cargo.toml` version bumped to the new vX.Y.Z; CHANGELOG.md updated if one exists.
  - **Cut the release**: `git tag vX.Y.Z`, `git push --tags`. Workflow auto-publishes.
  - **Pre-release naming**: `vX.Y.Z-rc1`, `vX.Y.Z-dev`, `vX.Y.Z-beta.2`, etc. The dash auto-flags as pre-release.
  - **After publish**: edit the release notes on GitHub if the auto-generated changelog needs annotation.
  - **Verification**: cite the install-script's checksum-verification step as the consumer of the `.sha256` files.

## 3. Documentation update (folded under existing project-documentation rule)

- [x] 3.1 Update README "Deployment" section (currently § "Deployment" near the bottom). Add a short subsection at the top of Deployment: **"Recommended: install from a binary release"** with a one-line summary pointing at the curl one-liner install path (described elsewhere in README under the existing "Quick install" section); the Deployment section itself covers source builds and manual installs for operators who need them. This spec's README change just frames the source-build content as the manual/advanced path; the actual "Quick install" content already landed via the install-script work.
- [x] 3.2 Add a short note in README explaining how releases are versioned and how to find them on the GitHub Releases page.

## 4. Spec delta

- [x] 4.1 Author the ADDED requirement "Tagged releases produce architecture-specific binaries on GitHub Releases" under `project-documentation` per the proposal.

## 5. Verification

- [x] 5.1 `openspec validate release-pipeline-github-actions --strict` passes.
- [x] 5.2 The workflow file includes the `lint` job that runs `rhysd/actionlint@v1` (see task 1.2). No host-side actionlint install is required: the linter runs as part of every CI execution of the workflow, including the workflow's own first run after this change merges. If the workflow itself contains an actionlint-detectable error, the lint job fails and the maintainer fixes via a follow-up change — same loop as any other CI failure.
- [x] 5.3 The workflow exposes a `workflow_dispatch:` trigger with a `dry_run: bool` input (default `true`) so the maintainer can manually exercise the matrix from the Actions tab without pushing a throwaway tag. When invoked via `workflow_dispatch` with `dry_run: true`, the `lint` + `test` + `build` stages run AND the `publish` stage is skipped via the `if:` expression on the publish job (see task 1.6). This replaces the previous "push a smoke-test tag" task, which autocoder's sandbox cannot perform.
