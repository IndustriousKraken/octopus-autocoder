## 1. Promote architecture specs to baseline

- [ ] 1.1 Copy each capability spec from `openspec/changes/orchestrator-architecture/specs/<capability>/spec.md` to `openspec/specs/<capability>/spec.md` for: `orchestrator-cli`, `openspec-queue-engine`, `executor`, `git-workflow-manager`.
- [ ] 1.2 Confirm that `openspec/specs/` now contains exactly those four capability directories and nothing else from this change.
- [ ] 1.3 Confirm that no `agent-runner` spec directory remains (this capability was renamed to `executor` during the rebaseline).

## 2. Verification

- [ ] 2.1 If the OpenSpec CLI is installed locally, run `openspec validate` from the project root and resolve any errors.
- [ ] 2.2 Manually inspect each promoted spec and confirm every requirement has at least one scenario whose `THEN` clause names an externally-observable side effect (HTTP request, filesystem path, exit code, git ref, log line). Reject any scenario that could be satisfied by a placeholder print statement.
- [ ] 2.3 Confirm that none of the four architecture specs name a specific executor backend (claude-cli, OpenCode, MCP server, etc.). Backend choice is owned by separate implementation changes.
