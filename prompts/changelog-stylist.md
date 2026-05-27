You are writing release notes for a project that uses OpenSpec. Your input
is a JSON object listing the archived changes shipped in this release
window AND the corresponding `proposal.md` files (read them with the
`Read` tool when you need fuller context than the JSON summary provides).

## Input

A JSON document with this shape:

```json
{
  "version": "<release version>",
  "date": "<YYYY-MM-DD>",
  "since": "<lower bound label>",
  "to": "<upper bound label>",
  "entries": [
    {
      "slug": "<change-slug>",
      "archive_dir": "<absolute path to the archive directory>",
      "primary_capability": "<capability name or null>",
      "summary": "<first paragraph of ## Why>",
      "shipped_commit": "<sha>",
      "shipped_date": "<YYYY-MM-DD>"
    }
  ],
  "skipped": [
    { "slug": "...", "reason": "..." }
  ]
}
```

The exact JSON data follows below the `## Deterministic data` heading.

When you need more context than the JSON summary provides, READ the
`<archive_dir>/proposal.md` file with the `Read` tool. The proposal's
full body explains motivation, trade-offs, and prior incidents — useful
for judging whether a change is headline-worthy, internal-only, or
something in between.

## Critical existence check

Before writing the changelog, check whether `CHANGELOG.md` exists in the
workspace root.

- If `CHANGELOG.md` IS present, READ it AND match its established style:
  heading hierarchy, item phrasing register, grouping convention, presence
  or absence of dates and PR links. Insert the new release's section in
  the correct chronological position (typically above the previous
  release, below any `## [Unreleased]` placeholder).
- If `CHANGELOG.md` is NOT present, CREATE it in the Keep a Changelog
  v1.1.0 format. The file MUST begin with:
  1. A top-level `# Changelog` heading (or the project's name).
  2. A brief explanatory paragraph linking to https://keepachangelog.com/en/1.1.0/.
  3. An `## [Unreleased]` placeholder section.
  4. This release's section: `## [<version>] - <YYYY-MM-DD>`.

## Register guidance

**Write release notes, not motivation paragraphs.** Each entry is one
sentence (two if the motivation is genuinely non-obvious). Lead with the
user-visible verb ("Adds X", "Fixes Y", "Changes Z behavior"). Drop
incident references, ticket numbers, and "we got burned last quarter
when..." context — those belong in proposals, not in the operator-facing
release notes.

## Grouping guidance

**Group thematically, not strictly by capability.** Related changes that
span capabilities should cluster ("Chat-driven workflows" rather than
splitting between `chatops-manager` and `orchestrator-cli`).

## Headline guidance

**Headline the release.** The top of the section gets 3-5 lead items —
the changes operators most want to know about. The long tail goes under
`### Also included` or analogous footer.

## Internal-only handling

Pure refactors, test-only changes, and doc-only changes belong in
`### Also included` OR you MAY propose `changelog: skip` frontmatter for
them. If 3+ entries are internal-only AND you decide they don't belong
in the changelog, propose `changelog: skip` frontmatter edits to the
relevant `openspec/changes/archive/<slug>/proposal.md` files in the
same commit. The frontmatter goes at the very top of the file, before
any other content:

```yaml
---
changelog: skip
---
```

Future releases inherit the decision automatically — the deterministic
extractor honors the frontmatter on subsequent runs.

## Output contract

- Write the polished changelog to `<workspace>/CHANGELOG.md` (creating
  or updating).
- You MAY also edit `openspec/changes/archive/<slug>/proposal.md` files
  to add `changelog:` frontmatter when a durable classification decision
  is implied by the operator's input.
- Do NOT touch any other path. Any other modifications will be rejected
  AND the diff refused.

## Deterministic data

{{changelog_json}}

## Repository context

Repository URL: {{repo_url}}

## Operator's instruction (if revising)

{{revision_text}}
