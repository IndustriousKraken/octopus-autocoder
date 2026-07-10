# Tasks

## 1. Spec delta — orchestrator-cli

- [ ] 1.1 Add requirement: `Roadmap items format` — defines the `roadmap/<slug>.md` file format, frontmatter schema (title, status, added), status vocabulary, and the rule that roadmap items are NOT queue-engine input.
- [ ] 1.2 Add requirement: `discuss-mode prompt routes future-feature ideas to roadmap items` — instructs the discuss artifact-creation agent that an aspirational/deferred/not-yet-specced feature idea SHALL produce a `roadmap/<slug>.md` rather than a full `openspec/changes/<slug>/` directory.

## 2. OCTOPUS.md — new section

- [ ] 2.1 Add a `## Roadmap items` section to `OCTOPUS.md` documenting: directory location (`roadmap/`), file format (frontmatter + free-text body), status values, creation via `@<bot> discuss` → `send it`, human-editable lifecycle (unlike openspec canon), and the rule that roadmap items are NOT automatically implemented.
- [ ] 2.2 The section SHALL state: "Roadmap items are searchable context — agents reading the workspace for a `discuss` or `audit` session SHOULD check `roadmap/` for relevant prior thinking."

## 3. roadmap/ directory

- [ ] 3.1 Create an empty `roadmap/` directory at the repo root (add a `.gitkeep` file so it is tracked).

## 4. Tests

- [ ] 4.1 Assert that `list_pending` (openspec-queue-engine) does NOT return entries from `roadmap/` — verifying that roadmap files are invisible to the queue engine without any new exclusion logic.
