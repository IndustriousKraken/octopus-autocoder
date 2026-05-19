## 1. Malformed-JSON dispatch error

- [ ] 1.1 `dispatch_returns_error_on_unparseable_json` (in
  `autocoder/src/control_socket.rs` tests module) — spin up
  `fixture_listener(BASE_YAML)`, send a request whose body is
  `not-json` (followed by `\n`), assert the parsed JSON response has
  `ok == false` AND `error` contains the substring `malformed JSON`.

## 2. Missing-action dispatch error

- [ ] 2.1 `dispatch_returns_error_when_action_field_missing` — spin
  up `fixture_listener(BASE_YAML)`, send the JSON body `{}`, assert
  the response has `ok == false` AND `error` contains the
  substring `missing` AND `action`. (One test covers both the
  no-fields case and the wrong-fields case by virtue of the
  production check only inspecting `parsed.get("action")`.)
- [ ] 2.2 Same test — also send `{"unrelated":"x"}` and verify it
  produces the same error response shape, locking in that the
  check is "missing action" and not "JSON has any field".
