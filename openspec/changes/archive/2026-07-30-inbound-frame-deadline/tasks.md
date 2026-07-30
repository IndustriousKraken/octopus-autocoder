# Tasks

- [x] Give the Slack event loop a frame deadline: when 90 seconds pass without
  any WebSocket frame arriving (event, ping, hello, disconnect, or other),
  exit the loop exactly as a stream error would — close the stream and return
  through the existing exit path so backoff and reconnect behavior are
  unchanged.
- [x] Make the disconnect log line name the frame deadline as the reason
  (distinct from ordinary Slack-initiated refreshes) so operators can spot a
  reaped half-open connection in the journal.
- [x] Unit test: a connected stream that yields nothing for the deadline
  window produces the stream-error exit, and the reported reason names the
  frame deadline.
- [x] Unit test: frames arriving with gaps shorter than the deadline —
  including bare protocol pings carrying no event payload — keep the
  connection alive past the point where an idle stream would have been
  reaped.
- [x] Unit test: cancellation still wins immediately while the loop is
  waiting inside the deadline window (exit within 1 second, clean close).
