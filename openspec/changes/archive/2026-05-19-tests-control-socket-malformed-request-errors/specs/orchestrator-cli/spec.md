## ADDED Requirements

### Requirement: Control socket rejects malformed requests with a named error
The control socket's `dispatch_request` SHALL respond with `{"ok": false, "error": "<message>"}` (the same envelope used for `unknown action`) when the incoming line cannot be turned into an `{action: ...}` request. The error message SHALL distinguish "the line was not JSON" from "the line was JSON but had no action field" so an operator running `nc -U <socket>` from a shell can tell whether the typo is in their JSON syntax or their field name.

#### Scenario: Request line is not valid JSON
- **WHEN** the daemon's control socket receives a line whose body is not valid JSON (e.g. `not-json\n`)
- **THEN** the response is a single JSON object with `ok == false` AND `error` containing the substring `malformed JSON`
- **AND** the connection is closed after the response is written

#### Scenario: Request JSON parses but lacks an `action` field
- **WHEN** the daemon's control socket receives a line whose body parses as a JSON object that has no `action` field (e.g. `{}` or `{"unrelated":"x"}`)
- **THEN** the response is a single JSON object with `ok == false` AND `error` containing the substrings `missing` AND `action`
- **AND** the response error is distinguishable from the `malformed JSON` error so log triage can tell typo-in-syntax from typo-in-field-name
