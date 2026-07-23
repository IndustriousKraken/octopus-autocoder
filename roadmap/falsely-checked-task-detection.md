---
title: Detect implementer tasks checked [x] without the work actually done
status: deferred
added: 2026-07-23
---

Observed once (2026-07-11, `a01-roadmap-items`): the implementer checked task 1.2
("Document the roadmap convention in OCTOPUS.md") but the documentation never
existed in any commit — plausibly written and then reverted by the OCTOPUS.md
provisioner, or never written at all. The acceptance scan only rejects
*unchecked* tasks, so a falsely-checked task passes silently; the advisory
`[out]` gate did not catch it either.

Candidate fix if it recurs: an `[out]`-gate prompt emphasis — verify that
checked documentation/file-creation tasks correspond to actual diff content,
not just that code tasks were implemented.

Deliberately deferred: one occurrence in 250+ archived changes. Will do if it
actually occurs often enough to matter. If you hit a second case, note it here
and promote this to a change.
