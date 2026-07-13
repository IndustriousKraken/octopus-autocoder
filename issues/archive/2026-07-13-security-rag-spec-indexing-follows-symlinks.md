# RAG spec indexing follows symlinks — daemon-side arbitrary file read disclosed to the agent

## Symptom

When canonical-RAG is enabled, the daemon indexes each managed repo's
`openspec/specs/<cap>/spec.md`. The discovery walk follows symlinks, so a repo
that commits `spec.md` as a symlink to an absolute host path (e.g.
`/etc/autocoder/secrets.env`, `~/.ssh/id_ed25519`, `/etc/passwd`) causes the
daemon (unsandboxed, owner privileges) to read that host file, embed it, and hand
its contents back to the sandboxed agent via the `query_canonical_specs`
control-socket action. This crosses the daemon→sandbox boundary and can leak
files the sandbox otherwise forbids.

## Why

`discover_canonical_specs` (`rag/mod.rs:420-438`) tests the candidate with
`spec_path.is_file()`, which FOLLOWS symlinks:

```
let spec_path = entry.path().join("spec.md");
if spec_path.is_file() {
    out.push(spec_path);
}
```

`rebuild_capabilities` (`rag/mod.rs:~260`) has the same pattern. The collected
paths are read with `std::fs::read_to_string` (`rag/chunking.rs:38`) in the
daemon process, chunked, embedded, and returned as `hits` (chunk source text) by
`handle_query_canonical_specs` (`control_socket/handlers.rs:~2446`). `git clone`
recreates a committed symlink verbatim in the working tree, and an absolute
target is checked out literally — so the attacker fully controls the target path.

Preconditions: `canonical_rag` enabled, and the indexed repo contains an
attacker-influenced commit (a contributor PR, or the agent itself planting the
symlink in-workspace before an on-archive re-embed). The codebase already has the
correct no-follow pattern elsewhere (`workspace_cache.rs:90-107` uses
`symlink_metadata`), so this is an inconsistency, not a missing capability.

## Tasks

- [ ] In `discover_canonical_specs` and `rebuild_capabilities`, reject symlinks:
  use `symlink_metadata()?.file_type().is_symlink()` and skip, OR `canonicalize()`
  the resolved `spec.md` and assert it `starts_with(workspace.join("openspec/specs"))`
  before reading. Reuse the `symlink_metadata` no-follow pattern already in
  `workspace_cache.rs`.
- [ ] Apply the same guard to any other repo-relative file the RAG indexer reads
  from a directory walk.

## Tests

- [ ] A `spec.md` that is a symlink (to an in-repo file and to an absolute
  out-of-tree path) is skipped by `discover_canonical_specs` — its contents never
  enter the index.
- [ ] A regular `spec.md` is still indexed (no regression).
