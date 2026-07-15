## ADDED Requirements

### Requirement: Ask-user fallback marker never fabricates change directories
When the per-execution stdio MCP server (`autocoder mcp-ask-user-server`) cannot relay an `ask_user` question over the control socket and falls back to writing a pending-question marker, the marker write SHALL NOT create any directory. A session whose `ORCH_MCP_CHANGE` value names an EXISTING `<workspace>/openspec/changes/<change>/` directory SHALL write `.askuser-pending.json` inside that directory (the existing placement for implementer sessions working a real change). Every other session — verifier gates, audits, the changelog stylist, and any session whose change key is a role name rather than a change (or whose change directory no longer exists) — SHALL write the marker to the workspace root as `.askuser-pending-<sanitized-key>.json`, where `<sanitized-key>` is the `ORCH_MCP_CHANGE` value restricted to `[A-Za-z0-9_-]` (other characters replaced with `-`). Marker writes remain atomic tempfile + rename in both placements.

The placement rule depends only on the workspace root existing, so it behaves identically in a daemon-managed workspace on a server and in a directly-cloned repository on a spec-authoring machine running `autocoder verify`. A fabricated `openspec/changes/<role>/` directory is never created, so `openspec list` and the queue engine never see a phantom change from an ask-user fallback.

#### Scenario: A gate session's fallback marker lands at the workspace root
- **WHEN** a verifier-gate session (e.g. `ORCH_MCP_CHANGE=canon_contradiction_check`) calls `ask_user` AND the control socket is unreachable
- **THEN** the MCP server writes `<workspace>/.askuser-pending-canon_contradiction_check.json` atomically
- **AND** no directory is created under `openspec/changes/`
- **AND** `openspec list` in that workspace shows no `canon_contradiction_check` change

#### Scenario: An implementer session's fallback marker stays in its change directory
- **WHEN** an implementer session whose `ORCH_MCP_CHANGE` names an existing `openspec/changes/<change>/` directory calls `ask_user` AND the control socket is unreachable
- **THEN** the marker is written to `openspec/changes/<change>/.askuser-pending.json` exactly as before
- **AND** the write is atomic tempfile + rename

#### Scenario: A change key with hostile characters is sanitized
- **WHEN** a session's `ORCH_MCP_CHANGE` contains path separators or other characters outside `[A-Za-z0-9_-]` AND its change directory does not exist
- **THEN** the root marker's filename uses the sanitized key (offending characters replaced with `-`)
- **AND** the marker cannot escape the workspace root

#### Scenario: Identical behavior on server and spec box
- **WHEN** the same gate session hits the fallback in a daemon-managed workspace AND in a directly-cloned repository under `autocoder verify`
- **THEN** both write the same root-relative marker path
- **AND** neither creates an `openspec/changes/` entry
