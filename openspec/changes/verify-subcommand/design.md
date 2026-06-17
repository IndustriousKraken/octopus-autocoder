# Design

## D1 — Reuse the exact check logic, not a reimplementation

The whole value is fidelity: `verify` must catch what the server catches. So it
invokes the SAME functions the verifier-gate framework runs — the `[in]` and
`[canon]` agentic-session checks (`verifier_gate.rs` / `llm.rs`), with the same
embedded prompts, the same `executor.change_*_contradiction_check_llm` model
config, and the same `submit_contradictions` / `submit_canon_contradictions`
submission schemas. It is a new *invocation surface* for those checks, not a second
implementation. (A separate implementation would re-introduce the reviewer-vs-gate
gap we keep hitting — an approximation that disagrees with the real gate.)

## D2 — Working-tree input, run in the repo

`verify <change-slug>` runs in the repository's working directory and reads
`openspec/changes/<change-slug>/specs/**` (the deltas) and the local
`openspec/specs/**` (canon) — the same files the server reads, but the local
working copy, before any push. This mirrors `openspec`'s own ergonomics (run in the
repo). No `--repo` selector is needed; the cwd repo is the target.

## D3 — Read-only; report, don't mark

Locally there is no queue, so there is nothing to exclude: `verify` does NOT write
`.needs-spec-revision.json`, does NOT run the executor, and does NOT modify the
workspace. It prints findings to stdout — grouped by gate, each labeled with the
gate identifier (`[in]` / `[canon]` / …) and carrying the same finding narrative the
server marker's `revision_suggestion` would — so the operator fixes the spec and
re-runs.

## D4 — Which gates run, and fail-closed

By default `verify` runs the gates ENABLED in config, so its verdict matches what
the server will actually enforce (running a gate the server has disabled would
report a "failure" the server would never raise). A selector overrides for explicit
use: `--all` runs every realized spec-checking gate; `--gate in,canon` runs a named
subset.

Exit semantics, fail-closed (per `gatekeepers-fail-closed`): 0 only when every gate
that ran returned no findings; non-zero when any gate finds a contradiction; AND
non-zero when an enabled gate CANNOT run (model unconfigured, transport error,
unregistered strategy) — `verify` reports "gate could not run" and fails, never
reports clean for a gate that did not actually evaluate. This makes it safe as a
pre-push hook or CI step: a green `verify` means the change was actually checked.

## D5 — Distribution to the spec-box

`verify` is a subcommand of the autocoder binary, so the binary it ships in is the
same one the server runs — guaranteeing identical logic. But the spec-authoring
machine is low-powered (it should never compile the Rust binary). The check-only
install therefore fetches a PREBUILT binary (built in CI / on the server), places
it on the interactive `PATH`, and drops a minimal config containing only what
`verify` needs: the `executor.change_*_contradiction_check_llm` model blocks (and,
later, the rule-corpus locations). No repos, chatops, reviewer, or daemon config is
required — `verify` operates on the cwd repo and the model endpoint.

## D6 — Relationship to the server gates and to global-rules

`verify` is the local accelerator; the server gates stay on as the fail-closed
enforcement (fresher canon at implement time, all contributors). They are feedback
vs. enforcement, not redundant. `verify` runs "the enabled spec-checking gates"
generically, so when the global-rules gate (a corpus-parameterized sibling of
`[canon]`) is added, `verify` picks it up with no change here.
