# Releasing autocoder

Tagged releases publish pre-built binaries via `.github/workflows/release.yml`.
Push a `vX.Y.Z` tag and the workflow tests, builds three target binaries, and
attaches them (plus `.sha256` files) to a new GitHub Release at that tag.

## Pre-flight

- `cargo test --release --all-features` is green on `main`.
- `autocoder/Cargo.toml` `version` is bumped to the new `X.Y.Z`.
- `CHANGELOG.md` updated if one exists.

## Cut the release

```bash
git tag vX.Y.Z
git push --tags
```

Watch the run in the Actions tab. On success the tag's GitHub Release is
created automatically with generated release notes and the six assets:

- `autocoder-vX.Y.Z-x86_64-unknown-linux-gnu` (+ `.sha256`)
- `autocoder-vX.Y.Z-aarch64-unknown-linux-gnu` (+ `.sha256`)
- `autocoder-vX.Y.Z-aarch64-apple-darwin` (+ `.sha256`)

## Pre-release tags

Tags containing a dash suffix are auto-flagged as pre-release on GitHub:
`vX.Y.Z-rc1`, `vX.Y.Z-dev`, `vX.Y.Z-beta.2`. The build is identical; only
the release's `prerelease: true` flag differs, which hides the release from
"latest" lookups and lets the install script's production-only filter skip it.

## After publish

Edit the release notes on GitHub if the auto-generated changelog needs
annotation (breaking changes, migration notes, highlights).

## Verification

Each binary's `.sha256` file is in `sha256sum -c` format. The `install.sh`
bootstrap script (see [Deployment](README.md#deployment)) downloads the
binary and checksum file and runs `sha256sum -c` (or `shasum -a 256 -c` on
macOS) before installing — a failed checksum aborts the install.
