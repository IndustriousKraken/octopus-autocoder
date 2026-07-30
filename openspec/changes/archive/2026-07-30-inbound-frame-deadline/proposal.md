# Inbound frame deadline

## Why

Production incident (2026-07-29): after an OS upgrade + reboot on the daemon
host, the Slack Socket Mode connection went half-open — the peer was gone but
no FIN/RST ever reached the daemon. The event loop's only wakeups are the
cancel token and `stream.next()`, so it pended forever on a read that would
never complete: the daemon logged `slack inbound: connected` once and then
nothing for 9+ hours, posted outbound alerts normally the whole time, and
silently ignored every operator command — including two `send it` replies in a
needs-revision thread. The reconnect loop never regained control because no
stream error ever surfaced. Only a daemon restart recovered inbound.

Slack pings healthy Socket Mode connections every few seconds and refreshes
them every few hours, so a connection that delivers no frame at all for even
one minute is dead. The event loop needs a liveness guard so a half-open
connection becomes a ~90-second blip (existing reconnect machinery) instead of
a silent, unbounded inbound outage.

## What Changes

- The Slack event loop gains a frame deadline: when NO WebSocket frame of any
  kind (event, ping, hello, disconnect, or other) has arrived for 90 seconds,
  the listener treats the connection as dead — exactly like a stream error —
  closes the stream, and re-enters the existing `apps.connections.open` +
  connect cycle with backoff.
- The disconnect log line names the frame deadline as the reason, so an
  operator reading the journal can distinguish "half-open connection reaped"
  from ordinary Slack-initiated refreshes.
- No configuration knob: Slack's ping cadence (seconds) makes 90 seconds
  generous with no false-positive risk, and the constant is trivial to revisit
  if Slack's protocol behavior ever changes.

## Capabilities

### MODIFIED: `chatops-manager`

The "Slack Socket Mode connection lifecycle" requirement gains the frame
deadline as a third connection-ending condition (alongside cancel and stream
error) and a scenario covering half-open-connection detection.

## Impact

- `autocoder/src/chatops/slack.rs`: `run_event_loop` gains a deadline on the
  frame read (a timeout around the read, or an equivalent `select!` arm); the
  timeout path maps to the existing `EventLoopExit` semantics so backoff and
  reconnect behavior are unchanged.
- No config, CLI, or state-file changes. Other backends are unaffected (the
  deadline lives in the Slack event loop, not the trait).
