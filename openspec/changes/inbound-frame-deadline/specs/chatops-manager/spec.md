# chatops-manager (delta)

## MODIFIED Requirements

### Requirement: Slack Socket Mode connection lifecycle
The Slack inbound listener SHALL obtain a WebSocket URL via `POST https://slack.com/api/apps.connections.open` using the configured app-level token, connect via WebSocket, and remain connected until the daemon's `cancel` fires, the stream errors, or the frame deadline elapses. A connection on which no WebSocket frame of any kind (event, ping, hello, disconnect, or other) has arrived for 90 seconds SHALL be treated as a stream error: the listener closes the stream and re-enters the reconnect cycle. (Slack pings healthy Socket Mode connections every few seconds, so 90 seconds of frame silence indicates a dead or half-open connection that will never error on its own.) On stream error or Slack `disconnect` envelope, the listener SHALL reconnect with exponential backoff starting at 1 second, doubling, capped at 30 seconds. A successful event roundtrip SHALL reset the backoff to 1 second. On cancel, the listener SHALL close the WebSocket cleanly and return.

#### Scenario: apps.connections.open is called with the app-level token
- **WHEN** the listener starts
- **THEN** it issues `POST https://slack.com/api/apps.connections.open` with `Authorization: Bearer <app_token>`
- **AND** on `ok: true` parses the response's `url` field as the WebSocket URL
- **AND** on `ok: false` returns an error whose text contains the Slack `error` field verbatim

#### Scenario: Disconnect envelope triggers reconnect with backoff
- **WHEN** the listener receives a `{"type":"disconnect", ...}` envelope
- **THEN** the listener closes the current stream
- **AND** waits `backoff_secs` (starting at 1, doubling on each successive failure)
- **AND** issues a new `apps.connections.open` + connect cycle

#### Scenario: Successful event resets the backoff
- **WHEN** the listener has reconnected after one or more failures and successfully processes at least one event
- **THEN** the next reconnect after a future disconnect waits 1 second, not the doubled previous backoff

#### Scenario: Backoff caps at 30 seconds
- **WHEN** the listener has experienced enough consecutive failures that `1 * 2^N` would exceed 30
- **THEN** the wait is capped at 30 seconds

#### Scenario: Cancel exits within 1 second
- **WHEN** the daemon's root cancel token fires while the listener is connected to Slack
- **THEN** the listener closes the WebSocket within 1 second and its `JoinHandle` resolves

#### Scenario: Frame deadline reaps a half-open connection
- **WHEN** the connected stream delivers no frame of any kind for 90 seconds
- **THEN** the listener treats the connection as dead, exactly as if the stream had errored
- **AND** closes the stream and re-enters the `apps.connections.open` + connect cycle with backoff
- **AND** the disconnect log line names the frame deadline as the reason

#### Scenario: Any frame within the window keeps the connection alive
- **WHEN** frames arrive on the connected stream (including bare protocol pings with no event payload) with gaps shorter than 90 seconds
- **THEN** the frame deadline never fires and the connection persists as before
