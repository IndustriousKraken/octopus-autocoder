# CLI Reference

```
autocoder <COMMAND>
```

## `run`

Start the polling daemon.

```bash
autocoder run --config <path-to-config.yaml>
```

The daemon polls every configured repository on its interval, processes ready OpenSpec changes, and opens monolithic PRs. Terminates only on SIGINT, SIGTERM, or a fatal initialization error. Logs go to stderr; control verbosity with `RUST_LOG=info` (default), `RUST_LOG=debug`, etc.

If a chatops backend is configured, the daemon posts a one-line `🆙 autocoder vX.Y.Z started` notification on every successful startup. Operators tracking unattended-update transitions watch this line in chat.

## `install`

First-run wizard / re-install entry point. The `install.sh` bootstrap swaps the binary then execs `autocoder install`; on an existing install with the systemd unit loaded, the subcommand short-circuits with the three-verb status block (update, reconfigure, wipe).

```bash
autocoder install [--reconfigure <section>] [--upgrade] [--non-interactive ...]
```

**`--reconfigure <section>`** re-prompts ONE section of an existing install and patches the existing `config.yaml`. Accepted values:

- `audits` — re-prompts every audit cadence with the operator's current cadence as the default, then writes the new `audits.defaults.*` in place via atomic temp-file-then-rename.
- `reviewer` — re-prompts provider, model, and api-key env-var, then shows a unified diff against the current file AND prompts `Apply this patch? [y/N]`. The patch lands only on `y/Y`.
- `chatops` — re-prompts the backend and default channel id, then diff-confirms the same way as `reviewer`.

The flag is mutually exclusive with `--non-interactive` AND with every prefill flag (`--repo-url`, `--token-env-var`, etc.) — reconfigure is interactive and section-scoped by definition. clap rejects the combination at argument-parse time.

Values not in the accepted list are rejected (e.g. `--reconfigure repositories` exits non-zero with the standard `possible values: audits, reviewer, chatops` clap error). The wizard intentionally excludes several knobs: `repositories` (use `autocoder reload` which hot-applies add/remove without a restart), `paths.*` (destructive, restart-required), `executor.*` (restart-required), and `audits.settings.*.prompt_path` / `audits.settings.*.extra.*` (advanced overrides — edit YAML directly).

After a successful patch, the subcommand prints `Patched <section> in <path>. To apply: sudo -u autocoder autocoder reload`. The wizard does NOT auto-reload — the operator decides when to apply.

If neither the systemd probe nor `<default-config-dir>/config.yaml` resolves to an existing file, `--reconfigure` exits non-zero with `no existing install detected; run install.sh for first-time setup`.

**`--issues-lane <enabled|disabled>`** (non-interactive) opts into the issues lane (`features.issues.enabled`). The default is `disabled`, mirroring the interactive yes/no gate the wizard shows after the periodic-audits prompts; an install that omits the flag produces the same lane-off config, so existing IaC is unaffected. Enabling the lane is a deliberate choice — it changes daemon behavior autonomously. See [CONFIG.md → features.issues](CONFIG.md#featuresissues).

## `check-config`

Validate a config file against this binary's schema, without starting the daemon. Runs the same pipeline `autocoder run` executes at startup (YAML parse, schema validation, token-route resolution, workspace-collision check, audit-slug validation, path-collision check, secret-source check) and reports the outcome to stdout / stderr.

```bash
autocoder check-config --config <path-to-config.yaml> [--json]
```

`--config <path>` is **required**. The subcommand has no global config-default resolution — every invocation names the file explicitly.

**Intended audiences:**

- **Operators editing YAML by hand.** Ask "does this file parse and validate?" without standing up the full daemon against your live repositories.
- **Scripted preflight.** The cron-updater (landing in a follow-up change as `update.sh`) calls `autocoder check-config --json --config /etc/autocoder/config.yaml` against the new binary before swapping it in. A non-zero exit aborts the swap and avoids the `Restart=on-failure` loop that a schema regression would otherwise cause.

**Exit codes:**

| Code | Meaning                                                       |
|------|----------------------------------------------------------------|
| `0`  | Config parses and every check passes with zero findings.       |
| `1`  | Config is valid but at least one WARN-level finding (typically an unset env var referenced by a `*_env` field). |
| `2`  | At least one hard error (parse failure, schema violation, unresolvable token route, workspace collision, audit-slug typo, path collision). |

**Default (human-readable) output.** One line per check on stdout, prefixed `OK:` / `WARN:` / `ERROR:`. Each finding's `config_pointer` (a JSON-Pointer-style locator into the YAML, e.g. `repositories/0/poll_interval_sec`) is appended in parentheses when present. On exit 1 or exit 2, stderr carries a one-line summary: `check-config: <N> error(s), <M> warning(s) in <path>`.

**`--json` output.** One JSON object per line on stdout. Each finding has the shape `{"level":"error"|"warn"|"ok", "category":"<slug>", "message":"<text>", "config_pointer":"..."|null}`. The final line is `{"level":"summary","errors":N,"warnings":M,"config":"<path>"}`. Every line is independently parseable, suitable for `jq` pipelines.

**Example — exit 0 (valid config).**

```bash
$ autocoder check-config --config /etc/autocoder/config.yaml
OK: schema — all required fields present and value ranges respected
OK: token-route — every repository has a resolvable GitHub token route
OK: workspace-collision — every repository resolves to a distinct workspace path
OK: audit-slug — every audit slug names a registered audit type
OK: path-collision — every paths.* role resolves to a distinct directory
OK: secret-source — every referenced env-var-sourced secret is set
$ echo $?
0
```

Stderr is empty.

**Example — exit 1 (warnings only, unset env var).**

```bash
$ autocoder check-config --config /etc/autocoder/config.yaml
OK: schema — all required fields present and value ranges respected
OK: token-route — every repository has a resolvable GitHub token route
OK: workspace-collision — every repository resolves to a distinct workspace path
OK: audit-slug — every audit slug names a registered audit type
OK: path-collision — every paths.* role resolves to a distinct directory
WARN: secret-source: github.token_env references `GITHUB_TOKEN` which is not set in the calling environment (github/token_env)
$ echo $?
1
```

Stderr:

```
check-config: 0 error(s), 1 warning(s) in /etc/autocoder/config.yaml
```

The WARN does not block: a config with only warnings still exits 1 (never 2). Unset env vars are common when the secrets are injected by systemd via `EnvironmentFile=` at unit start; running `check-config` from a shell without those vars set is the expected case.

**Example — exit 2 (schema violation).**

```bash
$ autocoder check-config --config /etc/autocoder/config.yaml
ERROR: schema: repositories[0].poll_interval_sec must be > 0 (got 0) (repositories/0/poll_interval_sec)
OK: token-route — every repository has a resolvable GitHub token route
OK: workspace-collision — every repository resolves to a distinct workspace path
OK: audit-slug — every audit slug names a registered audit type
OK: path-collision — every paths.* role resolves to a distinct directory
OK: secret-source — every referenced env-var-sourced secret is set
$ echo $?
2
```

Stderr:

```
check-config: 1 error(s), 0 warning(s) in /etc/autocoder/config.yaml
```

## `reload`

Ask a running daemon to re-read its YAML config and hot-apply the `github`, `reviewer`, and `chatops` sections.

```bash
sudo -u autocoder autocoder reload
```

The CLI connects to the daemon's Unix-domain control socket at `/tmp/autocoder/control/control.sock`, sends `{"action":"reload"}`, and prints the daemon's pretty-printed JSON response to stdout (exit 0) or stderr (exit non-zero). The socket file is mode `0600` and owned by the user the daemon runs as, so the CLI must run as the same user — hence `sudo -u autocoder`. If the daemon is not running, the CLI prints an error naming the expected socket path and exits non-zero. See [Runtime control: live config reload](OPERATIONS.md#runtime-control-live-config-reload) for the full behavior.

## `audit run`

Trigger an audit on-demand for one workspace, complementing the cadence-based scheduling that `autocoder run` does in the background.

```bash
# With the daemon running: enqueue the audit for the next polling iteration.
autocoder audit run --workspace /tmp/workspaces/github_com_acme_myrepo --audit security_bug_audit

# Without the daemon: invoke the audit module directly against the workspace
# and print findings to stdout. Useful for prompt-template iteration.
autocoder audit run --workspace /path/to/checkout --audit architecture_advisor
```

**`--audit`** is the exact `audit_type` slug (e.g. `security_bug_audit`, `drift_audit`, `architecture_advisor`). The chatops verb does substring matching against the operator's argument; the CLI does NOT — typing `--audit sec` is rejected with an `unknown audit` error listing the registered names. This is a deliberate asymmetry: a CLI call may be running inside a script where a substring match that suddenly resolves differently after a registry change would be surprising.

**Daemon-present path.** The CLI probes for the control socket at `/tmp/autocoder/control/control.sock`. When the socket is reachable, the CLI sends a `queue_audit` action with the workspace path; the daemon resolves the workspace to a managed repo and appends the audit-type to that repo's `pending_audit_runs` queue. The CLI prints the daemon's ack (`✓ Queued <audit> for <url>. Will run on the next polling iteration (~Nm).`) and exits 0. When the workspace is NOT in the daemon's repo list, the CLI prints an error naming the workspace and the daemon's known repos and exits non-zero — the CLI does NOT fall back to standalone mode in that case, because the daemon owns the workspace's lifecycle when present and a standalone invocation would race the daemon.

**Daemon-absent path.** When the socket is missing or refusing connections, the CLI builds a minimal audit registry, looks up the audit by name, constructs an in-memory `RepositoryConfig` whose `local_path` is `--workspace`, and invokes the audit's `run` directly. Findings (and any other outcome variant) are printed to stdout. This path skips the post-hoc write-policy enforcement the scheduler does, so use it only against a workspace you control and intend to inspect by hand.

Exit codes: 0 on success (queue ack OR standalone success), non-zero on any error (unknown audit, daemon refused the request, audit `run` errored, …).

## `changelog`

Harvest a release-notes changelog from the OpenSpec archive of a workspace. The subcommand walks `openspec/changes/archive/`, finds archive directories added between two git refs, pulls the first paragraph of `## Why` from each archive's `proposal.md` (or a frontmatter override), groups entries by primary affected capability, and emits markdown (default) or JSON to stdout.

```bash
# Default: changelog since the most recent tag on HEAD's ancestry,
# emitted as markdown to stdout.
autocoder changelog

# Bound the range explicitly.
autocoder changelog --since v0.3.0 --to v0.4.0

# First-release run when no tags exist yet:
autocoder changelog --since ever --to HEAD

# Run against a different OpenSpec checkout (or a daemon-managed workspace):
autocoder changelog --workspace /tmp/workspaces/github_com_acme_widgets
```

It is a pure-data extractor: no LLM involvement, no mutation, no daemon work, no external network. Same archive contents + same tag range produce the same output every invocation. The harvested prose is the operator's own — the `## Why` paragraphs already written in OpenSpec proposals.

**Flags:**

| Flag | Default | Purpose |
|------|---------|---------|
| `--workspace <path>` | current working directory | Directory containing `openspec/changes/archive/`. Set this to run the subcommand against a managed workspace from the daemon host (see [Workspace path derivation](OPERATIONS.md#workspace-path-derivation) for the `<cache_dir>/workspaces/<sanitized-url>/` path layout). |
| `--since <tag-or-sentinel>` | most recent tag on `HEAD`'s ancestry (`git describe --tags --abbrev=0 HEAD`) | Lower bound (exclusive). The literal value `ever` is a sentinel meaning "from the beginning of archive history" — explicit form for first-release runs. |
| `--to <tag-or-ref>` | `HEAD` | Upper bound (inclusive). |
| `--format markdown\|json` | `markdown` | Output shape. |

**No-tags fallback.** When `--since` is unset AND `git describe` exits non-zero (no tags exist on `HEAD`'s ancestry), the subcommand falls back to `ever` AND emits one INFO line to stderr:

```
No tags found in this repo; emitting full archive history. Pass --since ever to suppress this notice.
```

Exit code is `0` regardless — operators get useful output on the first-release case without inventing a fake tag. Passing `--since ever` explicitly suppresses the INFO line (the INFO line only fires under the implicit fallback path).

**Frontmatter overrides.** A change's `openspec/changes/<slug>/proposal.md` MAY carry YAML frontmatter that the extractor honors:

```markdown
---
changelog: skip                    # this change does not appear in the changelog
# OR
changelog: internal                # alias for `skip`
# OR
changelog: hidden                  # alias for `skip`
# OR
changelog:
  summary: "One-line override for the changelog entry"
---
## Why
...
```

- **No frontmatter** OR no `changelog:` field → default behavior: use the first paragraph of `## Why` as the entry summary.
- **`changelog: skip`** (or `internal`, `hidden`) → omit the change from the entries list AND record it in the JSON `skipped` array OR a markdown `### Skipped` footer.
- **`changelog: { summary: "..." }`** → use the override summary verbatim instead of the first-`## Why` paragraph.
- **Unrecognized value** → emit a WARN to stderr naming the value, then fall through to default behavior.

The frontmatter is harmless when no extractor reads it; pre-spec proposals work unchanged.

**Range filtering.** Archive discovery uses git addition commits (`git log --diff-filter=A ... -- openspec/changes/archive/`), NOT the directory name's date prefix. An archive appears in the output if and only if its addition commit is reachable from `--to` but not from `--since`. The directory's `YYYY-MM-DD` prefix is used only for the entry's `shipped_date` field, so a manually-renamed archive directory does not affect what changelogs include.

**Markdown output shape:**

```markdown
## v0.4.0 — 2026-05-28

### chatops-manager
- **Chat-driven proposals via `@<bot> propose`** (chat-request-triage) — Operators can now ask autocoder to act on a free-form request from chat. The agent classifies the request as DIRECTIVE, QUESTION, or AMBIGUOUS and produces a fixes PR and/or a spec PR.
- **Audit-finding triage via `@<bot> send it`** (audit-reply-acts) — Reply inside an audit's threaded findings to spawn a triage run.

### orchestrator-cli
- **Streaming JSON output capture** (executor-streams-output-incrementally) — Per-change logs gain PROMPT / ACTIONS / FINAL ANSWER / STDERR sections.

### Skipped
- `internal-only-refactor` — changelog: skip
```

Entries group under `### <capability>` where `<capability>` is the alphabetically-first directory under the change's `specs/` tree. The bullet form is `- **<summary-first-line>** (<slug>) — <rest-of-summary-if-any>`. The slug parenthetical is the archive directory's name with the date prefix stripped, so a reader can grep back to the source proposal. Archives without spec deltas group under `### Other`. Skipped entries (frontmatter `skip` / `internal` / `hidden`) appear in a `### Skipped` footer when at least one was skipped.

**JSON output shape:**

```json
{
  "version": "v0.4.0",
  "date": "2026-05-28",
  "since": "v0.3.0",
  "to": "HEAD",
  "entries": [
    {
      "slug": "chat-request-triage",
      "archive_dir": "/tmp/workspaces/.../openspec/changes/archive/2026-05-22-chat-request-triage",
      "primary_capability": "chatops-manager",
      "summary": "Chat-driven proposals via @<bot> propose...",
      "shipped_commit": "abc123def456...",
      "shipped_date": "2026-05-22"
    }
  ],
  "skipped": [
    { "slug": "internal-only-refactor", "reason": "changelog: skip" }
  ]
}
```

Pretty-printed with 2-space indents for human readability AND scripting alike. Downstream tooling (e.g. an LLM-stylist that produces a hand-edited `CHANGELOG.md`) consumes the JSON variant; the markdown variant is what the release workflow ships as the GitHub Release body.

**Cross-project usage.** The subcommand's only invariants are `openspec/changes/archive/` existing AND a git history in the workspace — it works against any OpenSpec checkout, not just autocoder's own repo. Two common patterns:

1. **From inside any OpenSpec checkout** (operator's laptop, a CI runner):

   ```bash
   cd /path/to/some-other-openspec-repo
   autocoder changelog --since v1.4.0
   ```

2. **From the daemon host pointing at a managed workspace** (no `cd` needed):

   ```bash
   sudo -u autocoder autocoder changelog \
       --workspace /tmp/workspaces/github_com_acme_widgets \
       --since v2.0.0
   ```

   The managed-workspace path layout under `<cache_dir>/workspaces/<sanitized-url>/` is documented in [Workspace path derivation](OPERATIONS.md#workspace-path-derivation).

**Release workflow integration.** `.github/workflows/release.yml` invokes `autocoder changelog --since <previous-tag> --to <new-tag>` between the test gate AND the publish step AND passes the output to the GitHub Release body. A failing changelog generation does NOT block the binary release — the workflow logs the error, writes an empty notes file, and continues. The binary upload is the primary artifact; notes are a best-effort enhancement.

For an LLM-styled draft that opens a PR for review, use the `@<bot> changelog` chatops verb instead. See [CHATOPS.md → Generating a changelog](CHATOPS.md#generating-a-changelog-changelog).

## `rewind`

Discard the in-flight agent branch and re-queue one or more archived changes. Use this when an agent produced unusable work or a PR was rejected and you want the daemon to try again.

```bash
# Soft rewind (single-repo config): prompt for confirmation, then delete
# the local agent branch and unarchive one change.
autocoder rewind my-broken-change --config config.yaml

# Hard rewind: skip the prompt, delete local AND remote agent branch,
# then unarchive two changes.
autocoder rewind change-A change-B --config config.yaml --hard

# Multi-repo config: --repo is REQUIRED. The selector matches either the
# full URL or the short-name (basename minus .git).
autocoder rewind my-change --config config.yaml --repo my-repo
```

**Soft vs hard semantics:**

| Mode     | Confirmation prompt | Local agent branch | Remote agent branch                       |
|----------|---------------------|--------------------|-------------------------------------------|
| soft     | y/N, defaults no    | deleted            | left intact                                |
| `--hard` | skipped             | deleted            | deleted (failures logged but non-blocking) |

The confirmation prompt for soft rewind looks like:

```
This will delete branch 'agent-q' (local) and unarchive 1 change(s) (my-broken-change). Proceed? [y/N]
```

Bare Enter, `n`, or any input other than `y`/`Y` declines and exits without modifying any state.

**`--repo` selector:**

With **one** configured repository, `--repo` is optional and defaults to that repo. With **two or more** configured repositories, `--repo` is required. autocoder matches the selector against each repository's full URL (exact equality) AND against the URL's short-name (basename with any trailing `.git` stripped). Zero matches or multiple matches exit non-zero with an error listing the available selectors.

**Unarchiving multiple changes:**

If you pass multiple change names and one of them fails to unarchive (typo, no matching archive entry, destination collision), the remaining names are still attempted. The process exits non-zero at the end with a summary naming both the succeeded and failed changes.

**Recovering from an accidental rewind:**

Archived directories are **not** deleted by archive — they are renamed under `openspec/changes/archive/<YYYY-MM-DD>-<name>/`. To reverse an accidental rewind, move the directory back into the archive manually. The canonical date-prefix format is preserved by autocoder's `archive` step, so a manual `mv` restores the queue's view of state.

---
