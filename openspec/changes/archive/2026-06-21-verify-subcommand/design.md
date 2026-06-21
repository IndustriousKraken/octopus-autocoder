# Design

## D1 — The submission transport is a hard precondition (the load-bearing fix)

The pre-executor gates capture the agent's verdict through an MCP `submit_*` tool.
The per-execution MCP child relays `record_submission` over a Unix control socket
(target = `mcp_askuser_server::ENV_CONTROL_SOCKET`) to a
`crate::submission_store::SubmissionStore` held in the daemon's `ControlState`. The
gate runner drains it via `crate::audits::try_consume_submission`, which reads that
env var and returns `None` when the socket is absent. The env var is set ONLY at
daemon startup (`cli/run.rs`). So a `verify` that just calls the gate functions
would get `None` from every drain and every gate would fail closed — `verify` would
NEVER pass.

`verify` therefore stands up the submission transport in-process, per invocation,
as a hard precondition. This is the exact sequence `cli/run.rs` already runs inline
at startup, so it is extracted into a shared helper,
`control_socket::spawn_submission_listener(paths)`, which (in order):

1. constructs `crate::submission_store::SubmissionStore::new()`;
2. registers the gate submission schemas
   (`preflight::change_contradiction::register_contradiction_submission_schema`,
   `preflight::canon_contradiction::register_canon_contradiction_submission_schema`,
   `preflight::global_rules::register_rule_violations_submission_schema`) and the
   audit schemas (`audits::register_submission_schemas`);
3. binds the socket (`control_socket::bind_at(&control_socket::socket_path(paths))`);
4. sets `ENV_CONTROL_SOCKET` to the socket path;
5. spawns `control_socket::serve(listener, path, state, cancel)` on a minimal
   `ControlState`;
6. returns a guard whose `Drop` cancels the `CancellationToken`, which stops `serve`
   and removes the socket file.

Without the listener, the gates fail closed — so the helper is a precondition, not
an optimization. A scenario asserts exactly this (env var absent → every gate
`Errored`).

## D2 — One schema-registration source (three callers), one daemon-absent listener (abstraction-first; the fold)

The same daemon-absent gap already makes `cli/audit.rs::run_standalone`
non-functional: it does zero socket bootstrap, so submission-based advisory audits
(architecture_advisor, drift, documentation, canon_contradiction) get `None` from
`try_consume_submission` and map it to a hard `Err` ("no submit_findings
submission"). This is fail-closed (never a false "0 findings") but means
`autocoder audit run <advisory>` cannot work without a daemon.

Two abstractions are shared rather than duplicated:

- `register_gate_and_audit_submission_schemas` — the gate + audit submission SCHEMA SET — is called by ALL THREE submission-capturing entry points (the daemon at startup, `verify`, AND `cli/audit.rs::run_standalone`); it single-sources the drift-prone schema set (a gate whose schema is registered in one path but not another silently fails to capture).
- `spawn_submission_listener` — the full bind → env-var → `serve` bootstrap on a submission-only `ControlState` — is shared by the two DAEMON-ABSENT callers (`verify` and `cli/audit.rs::run_standalone`). The daemon keeps its own bootstrap because it serves a FULL `ControlState` (github/reviewer/chatops/reload handlers) the submission-only helper does not build.

Extract the primitive once rather than duplicating the bootstrap or patching each
caller independently.

## D3 — Reuse the exact check logic, not a reimplementation

The whole value is fidelity: `verify` must catch what the server catches. So it
invokes the SAME entry points the verifier-gate framework runs —
`preflight::change_contradiction::run_agentic_contradiction_check` (`[in]`),
`preflight::canon_contradiction::run_agentic_canon_contradiction_check` (`[canon]`),
`preflight::global_rules::run_agentic_global_rules_check` (`[rules]`), with the
shared core in `preflight::corpus_check` — using the same embedded prompts, the same
per-gate model config (`executor.change_internal_contradiction_check_llm`,
`executor.change_canonical_contradiction_check_llm`,
`executor.global_rules_check_llm`), and the same submission schemas. It is a new
*invocation surface* for those checks, not a second implementation. (A separate
implementation would re-introduce the reviewer-vs-gate gap we keep hitting — an
approximation that disagrees with the real gate.) Note `verifier_gate.rs` is only
the registry/labels/retry helper and `llm.rs` only resolves the model; neither is
the check function.

## D4 — Working-tree input, run in the repo

`verify <change-slug>` runs in the repository's working directory and reads
`openspec/changes/<change-slug>/specs/**` (the deltas) and the local
`openspec/specs/**` (canon) — the same files the server reads, but the local working
copy, before any push. This mirrors `openspec`'s own ergonomics (run in the repo).
No `--repo` selector is needed; the cwd repo is the target.

## D5 — Scoped read-only; report, don't mark

Locally there is no queue, so there is nothing to exclude: `verify` does NOT write
`.needs-spec-revision.json`, does NOT run the executor, and does NOT make spec or
source edits. It is NOT, however, fully workspace-inert: standing up the listener
and the gate sessions creates transient run artifacts (`.mcp.json`, the control
socket). These are the only files `verify` touches, and they are cleaned up on exit
(the listener guard's `CancellationToken` removes the socket; the per-run MCP config
is removed like the daemon's). `verify` prints findings to stdout — grouped by gate,
each labeled with the gate identifier (`[in]` / `[canon]` / `[rules]`) and carrying
the same finding narrative the server marker's `revision_suggestion` would — so the
operator fixes the spec and re-runs.

## D6 — Which gates run, fail-closed, and no manufactured pass

By default `verify` runs the gates ENABLED in config, so its verdict matches what
the server will actually enforce (running a gate the server has disabled would
report a "failure" the server would never raise). A selector overrides for explicit
use: `--all` runs every realized spec-checking gate; `--gate in,canon` runs a named
subset.

Exit semantics (per `gatekeepers-fail-closed`): `0` only when every gate that ran
returned no findings; non-zero when any gate finds a contradiction; AND non-zero
when an enabled gate CANNOT run (model unconfigured, transport error, unregistered
strategy, no submission captured) — `verify` reports "gate could not run" and fails,
never reports clean for a gate that did not actually evaluate.

The empty case is loud, not silent (per `gatekeepers-contain-no-judgment`): if the
resolved gate set is empty — the config enables no spec-checking gate and no
selector forces one — `verify` does NOT exit 0. It reports that no gate evaluated the
change and exits non-zero. Code never manufactures a clean pass when nothing was
evaluated; a green `verify` means the change was actually checked.

## D7 — Timeout honors the unified config

`verify` resolves its agentic-session timeout from
`ExecutorConfig::agentic_session_timeout()` (which reads
`executor.agentic_session_timeout_secs`, default `3600` via
`default_agentic_session_timeout()`) — the same path the daemon, reviewer, and
`[out]` gate use. No verify-local literal; the unified timeout governs every agentic
session, including this one.

## D8 — Distribution to the spec-box

`verify` is a subcommand of the autocoder binary, so the binary it ships in is the
same one the server runs — guaranteeing identical logic. But the spec-authoring
machine is low-powered (it should never compile the Rust binary). The check-only
install therefore fetches a PREBUILT binary (built in CI / on the server), places it
on the interactive `PATH`, and drops a minimal config containing only what `verify`
needs: the gate model blocks (`executor.change_internal_contradiction_check_llm`,
`executor.change_canonical_contradiction_check_llm`,
`executor.global_rules_check_llm`), their `enabled` flags, and the rule-corpus
locations. No repos, chatops, reviewer, or daemon config is required — `verify`
operates on the cwd repo and the model endpoint.

## D9 — Relationship to the server gates

`verify` is the local accelerator; the server gates stay on as the fail-closed
enforcement (fresher canon at implement time, all contributors). They are feedback
vs. enforcement, not redundant. `verify` runs "the enabled spec-checking gates"
generically, so a later corpus-parameterized gate is picked up with no change here.
