# macOS add-on host ABI v1
Link the single archive produced by building package `backtracking-addon-host`. In
desktop/macos/build.sh change the package on line 20 and the archive basename on line 37
to libbacktracking_addon_host.a. Keep the existing Apple frameworks. Import
backtracking.h, backtracking_positions.h and backtracking_addon_host.h in the app bridging header.
The minimal C smoke test in examples/link.c needs no extra frameworks.

## Ownership and polling
Each handle owns an independent worker. Primary owns the data root, secondary proxies
the primary. Supply the absolute root and app version at creation. Compare generations
for inequality, then read the snapshot. The generation embedded in the snapshot is
the one to remember. Poll control and positions independently of snapshot changes.
Free releases the handle and signals shutdown. The worker sends quit, waits up to
two seconds and terminates remaining groups. Callers must synchronize final release
with all calls on that handle. Release each returned JSON string using bt_addon_string_free.

## Snapshot
All top-level JSON documents have integer schema 1. The snapshot has generation (u64),
ready (bool), error (string or null), addons (array), appearances (object keyed by
add-on id, values are protocol Appearance), completed (array of outstanding completed ticket ids).
Each addon has id, manifest (object or null), enabled, state, reason (string or null),
pid (u32 or null), message (null or {level,text}), sources, settings, actions,
control_allowed, control_asked. State values are stopped, starting, running, stopping,
not_responding, failed, broken_package. Each source is the v1 protocol Source including
sender, formats, format, optional groups and transport. Settings have declaration and value.
Protocol objects, spelling, units, omitted optional fields and control state follow
docs/ADDON_CONTRACT.md and crates/backtracking-addon/src/protocol.rs.

## Commands
Pass an object with schema=1, op and the fields below. Successful asynchronous admission
returns {"schema":1,"ok":true,"ticket":1}. On completion retrieve exactly once with
take_result. Successful completion returns {"schema":1,"ok":true,"value":null}
or a value described below. Failure returns {"schema":1,"ok":false,"code":"failed","message":"…"}.
A ticket is scoped to its handle. Before readiness callers may receive busy.

|op|additional fields or return value|
|---|---|
|inspect|path; returns manifest, program, file_count, entry_count, unpacked_size, review_id|
|install|path, review_id (64 lowercase SHA-256 hex characters); returns same summary without review_id|
|remove|id, delete_data (bool)|
|set_enabled|id, enabled (bool)|
|start, stop|id|
|set_setting|id, key, value|
|run_action|id, action|
|open_file|id, path|
|select_format|id, source, format|
|transport|id, source, command, optional position|
|clip_exported|files (array of paths), sample_rate, frames|
|set_control_allowed|id, allowed (bool)|
|take_control|returns array of {addon_id,addon_name,run,request_id,command}; command is a protocol ControlCommand|
|control_reply|addon_id, run, request_id, exactly one of result (ControlResult) or error ({code,message})|
|positions_since|cursor (u64, initially zero); returns {cursor,positions:[{sequence,addon_id,source_id,time,channels}]} with Sender clock nanoseconds|
|take_result|ticket (u64); immediate, returns busy if pending, not_found if unknown|

The protocol's six ordinary capabilities are filtered by the worker. Control requires
explicit per-id consent, retained on update and cleared on removal. After draining
take_control, re-check the current snapshot's grant immediately before executing. Late
replies are discarded. Maximum eight requests per add-on, deadlines ten seconds (save:
120 seconds). Control error codes are not_declared, not_allowed, invalid_request,
request_too_large, unknown_command, too_many_requests, duplicate_id, busy, not_found,
no_audio, no_selection, failed, timeout.

## Position tracks
One mutable core track per lane receives frame-indexed keys. Trim to the retained frame
range, freeze with the corresponding audio, then read immutable per-channel keys.
The archive exports the positions header. The frozen-audio consumer needs an additive
entry point to attach a cloned FrozenPositionTrack per lane before preview/export.
