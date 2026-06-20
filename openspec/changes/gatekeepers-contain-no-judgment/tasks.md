# Tasks

## 1. Extend the standard

- [ ] 1.1 MODIFY the `project-documentation` requirement "Control-plane gatekeepers fail closed, never to a passing verdict" to add the judgment-ownership dimension — the two clauses (verdict is the agent's, surfaced verbatim, no code-synthesized verdict; the agent is never given an option set that forecloses failure) AND the two scenarios — preserving every existing clause and scenario verbatim (this delta).

## 2. Record it for contributors

- [ ] 2.1 Extend the `CONTRIBUTING.md` "Control-plane gatekeepers fail closed" section with the judgment-ownership dimension: an agent-backed gatekeeper's code only initializes-to-failed, assembles inputs, invokes the agent, AND surfaces the agent's verdict verbatim; the code synthesizes no verdict (a code-authored "nothing to evaluate, so pass" is a manufactured pass), AND the verdict mechanism must let the agent express a failing verdict with prose alone (structured detail is never a precondition for failing). Keep the existing fail-closed content; add this as the second axis.

## 3. Conformance is separate (not in this change)

- [ ] 3.1 No runtime code here. Record the known non-conformances as their own changes, each conforming to the extended standard: the `[out]` gate's empty-delta manufactured pass; the reviewer verdict mechanism / surfacing; any gate whose option set or surfacing forecloses or hides a failing verdict.
