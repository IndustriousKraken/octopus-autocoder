# Decompose five accreted hotspots flagged by architecture_advisor

## Problem

The `architecture_advisor` audit anchored five oversized / low-cohesion
regions, each a "junk drawer" or duplicated-arm pile that has accreted
over time:

1. **`autocoder/src/control_socket.rs:1530-2422`** — ~10 `handle_queue_*`
   enqueue handlers share a near-identical body (require `url` +
   `request_id`, `find_repo`, look up the live `RepoTaskHandle` pending
   queue, de-dup by `request_id`, push, return `poll_interval_sec`), and
   the `dispatch_request` action router is one ever-growing
   `match action.as_str()`. The file is ~5,380 lines.
2. **`autocoder/src/chatops/operator_commands.rs:3491-4047`** — the
   four-context `send it` cascade and its audit/survey state-machine
   logic live inline in a ~10,200-line file.
3. **`autocoder/src/code_reviewer.rs:882-1588`** — the self-contained
   "Agentic reviewer transport (a58)" block sits inside the larger
   reviewer file with no module boundary.
4. **`autocoder/src/revisions.rs:621-1538`** — `process_one_pr` is a
   ~915-line function whose six executor-outcome match arms repeat the
   same post-processing shape.
5. **`autocoder/src/cli/install.rs:2126-2607`** — the entire
   `--reconfigure` subsystem is embedded in the install module.

These are maintainability signals, not defects: the line counts are the
audit's *selector*, never a contract. The work is a behavior-preserving
reorganization.

## Desired end state

Each region is decomposed/de-duplicated **with no observable behavior
change**: identical control-socket responses, identical chatops replies
and `send it` context ordering, identical reviewer output, identical PR
outcomes, and an identical CLI surface for `--reconfigure`. Public call
sites stay stable (re-export moved items as needed). The existing test
suite still passes, and any unit tests that move land in sibling test
modules rather than growing inline `#[cfg(test)]` blocks.

No new canonical requirement is authored from any size/duplication
threshold — the size budget keeps its single advisory home in
`project-documentation`'s `Source files and functions stay within a size
budget` requirement.
