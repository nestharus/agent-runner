# Local provider launch lifetime

`ProviderClient::launch` and the generic `invoke_json("launch", ...)` path
supervise local processes without either a stdout-silence deadline or a total
runtime deadline. Initial silence, gaps between events, and silence after a final
event are not evidence that a local agent has failed. Heartbeats are optional
protocol events, not permission to remain alive; no network liveness is assumed.

`ProviderTimeouts` has no launch budget, and `with_timeout` configures non-launch
operations only. Both launch paths explicitly select `TimeoutMode::NoDeadline`.
The shared `ProcessLimits::timeout` field is ignored in that mode, not increased
or replaced with a sentinel duration. `ProcessRunner::run` remains an explicitly
bounded low-level utility with a total-runtime budget; it is not the client
launch path. Non-launch handshake behavior is unchanged.

Deadline-free does not mean ownership-free: the supervisor still polls actual
process exit, accepts explicit cancellation with termination/escalation, handles
protocol/worker failures, and collects or retains cleanup ownership under the
existing bounded settlement rules. Output capture limits and process-status
poll cadence are unchanged. A final protocol event does not itself release the
process. A quiet process that never exits requires explicit cancellation; that
is intentional, not an automatically diagnosed outage.

Regression evidence lives in `launch_stream_lifecycle.rs` (quiet valid launch
outlives a short handshake budget, waits after its final event for actual exit,
and cancellation cleans descendants) and `age246_external_transport_rotation.rs`
(actual exit without a final event remains a protocol error, not a silence
timeout or permission to try a sibling account).
