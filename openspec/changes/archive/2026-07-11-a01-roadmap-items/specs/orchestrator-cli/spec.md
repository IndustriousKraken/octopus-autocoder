## ADDED Requirements

### Requirement: Roadmap items are a documented lightweight future-feature record
The repository SHALL support a `roadmap/` directory at its root that holds future-feature records, one per file at `roadmap/<slug>.md`, where `<slug>` matches `^[a-zA-Z0-9_-]{1,64}$`. Each record carries YAML frontmatter with `title` (a one-line description), `status` (one of `proposed`, `considering`, `planned`, `deferred`), and `added` (creation date, `YYYY-MM-DD`), followed by a free-text markdown body with no required sections. The convention SHALL be documented in `OCTOPUS.md`.

Roadmap items are operator- and agent-editable idea records. Unlike the canonical specifications under `openspec/specs/`, they are NOT autocoder-owned and carry NO gate enforcement; their lifecycle transitions (for example `proposed` → `considering` → `planned`, or `planned` → `deferred`) are made by editing the `status` field. They are NOT queue input: the queue engine enumerates only `openspec/changes/`, so roadmap files sit outside its scope by position AND no exclusion logic is required to keep them out of the implementation pipeline.

#### Scenario: A roadmap item is a single frontmatter-tagged file
- **WHEN** a future-feature idea is recorded under `roadmap/`
- **THEN** it is a single file `roadmap/<slug>.md` carrying `title`, `status`, and `added` frontmatter followed by a free-text body
- **AND** `status` is one of `proposed`, `considering`, `planned`, or `deferred`

#### Scenario: The roadmap convention is documented in OCTOPUS.md
- **WHEN** a contributor or agent needs to create or interpret a roadmap item
- **THEN** `OCTOPUS.md` documents the `roadmap/` location, the frontmatter format, the status values, the human- and agent-editable lifecycle, AND that roadmap items are never automatically implemented
