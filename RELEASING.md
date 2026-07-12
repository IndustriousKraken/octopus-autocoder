# Releasing autocoder

A release is a Git tag of the form `vX.Y.Z` pushed to the upstream remote. The `.github/workflows/release.yml` workflow picks the tag up, runs tests, builds binaries for three target triples, computes SHA-256 checksums, and publishes a GitHub Release with all six assets attached.

## Pre-flight

1. Confirm `main` is green: the CI/release workflow's `lint` + `test` stages must pass on the commit you intend to tag.
2. Bump the version in `autocoder/Cargo.toml` to the new `X.Y.Z`. Commit the bump on `main`.
3. If a `CHANGELOG.md` exists, update it (the GitHub Release body is auto-generated from commits, but a human-curated changelog stays the source of truth).

## Cut the release

```bash
git tag vX.Y.Z
git push origin vX.Y.Z
```

That's the whole maintainer-side ritual. The workflow handles test → matrix build → publish on its own. Expect ~15–25 minutes end-to-end.

## Pre-release naming

Tags with a dash suffix are auto-flagged as pre-releases by the workflow (`prerelease: ${{ contains(github.ref_name, '-') }}`). Use them for rcs, betas, and dev builds:

- `vX.Y.Z-rc1`
- `vX.Y.Z-beta.2`
- `vX.Y.Z-dev`

The install script's "production releases" filter skips pre-releases by default, so dev tags don't disturb operators who follow `latest`.

## Dry-run from the Actions tab

The workflow exposes a `workflow_dispatch` trigger with a `dry_run` boolean input (default `true`). Triggering it manually from *Actions → release → Run workflow* exercises `lint`, `test`, and `build` against any branch without creating a tag or publishing a release. Set `dry_run` to `false` only if you genuinely want a workflow_dispatch run to publish (the normal path is to push a tag).

## Migration notes

### `discuss` verb replaces one-shot `propose` (discuss-verb-conversational-propose)

The chatops `propose` verb is now a permanent alias for the new conversational
`discuss` verb, handled by a dedicated always-on discuss handler rather than the
polling loop. The old `.chat-reply.md` marker path is removed.

Before deploying this release, **drain any in-flight `ProposalRequestState`
files** so no request is stranded mid-lifecycle across the cutover: let the
running daemon finish one full polling cycle per repo (so `pending_proposal_requests`
queues empty and any `Pending`/`TriagePending` request reaches a terminal
status), or clear `<state_dir>/proposal-requests/` after confirming no request
is `Pending`/`TriagePending`. New `discuss` sessions persist to
`<state_dir>/discussions/` instead; those are safe to leave in place across the
upgrade. (The `proposal-requests` state type is retained for the internal
scout→`spec-it` flow, which still submits directive-shaped requests through the
retained spec-PR path.)

## After publish

The Release body is GitHub's auto-generated changelog. Edit it on the Release page to add highlights, breaking-change notes, or upgrade guidance if the auto-summary needs annotation.

## Verification

Each binary ships with a `.sha256` file in `sha256sum -c`-compatible format. The `install.sh` bootstrap downloads both and verifies before placing the binary on disk; operators who download manually can run `sha256sum -c autocoder-vX.Y.Z-<triple>.sha256` (or `shasum -a 256 -c …` on macOS) against the asset they pulled.
