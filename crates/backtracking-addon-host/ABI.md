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
take_control, positions_since and control_reply return the value envelope immediately
and do not allocate a ticket or advance generation. control_reply acknowledges admission
to the worker; the worker drops a stale run or expired request. A ticket is scoped to its handle. Before readiness callers may receive busy.

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

On a first install the host records the package as stopped. Request set_enabled=true
to enable and launch it. Updates preserve the saved enabled and control switches.
Only broken_package has a null manifest. For each source, sender matches the id from
bt_senders_string(...,name=0) on a bt_senders_new list. Optional transport contains state
(playing, paused or stopped), position (seconds), and optional duration (seconds).
clip_exported broadcasts completed file paths, frame count and sample rate to add-ons
declaring clips. Reply separately to a save control request with its ControlResult,
including state and SavedFile records, or with an error.

The protocol's six ordinary capabilities are filtered by the worker. Control requires
explicit per-id consent, retained on update and cleared on removal. After draining
take_control, re-check the current snapshot's grant immediately before executing. Late
replies are discarded. Maximum eight requests per add-on, deadlines ten seconds (save:
120 seconds). Control error codes are not_declared, not_allowed, invalid_request,
request_too_large, unknown_command, too_many_requests, duplicate_id, busy, not_found,
no_audio, no_selection, failed, timeout.

## Position tracks
BtFrozenPositions is the opaque C name for backtracking_core::FrozenPositionTrack.
bt_capture_clock_ns uses the same monotonic clock as positions_since. Align against
received packet timestamps and retained frames, accounting for pauses and dropped audio.
One mutable core track per lane receives frame-indexed keys. Trim to the retained frame
range, freeze with the corresponding audio, then read immutable per-channel keys.
The archive exports the positions header. The frozen-audio consumer needs an additive
entry point to attach a cloned FrozenPositionTrack per lane before preview/export.

Build the private process-group supervisor binary `bt-addon-guard` from the same
package and copy it beside the application executable (Contents/MacOS) before signing.
Each running add-on has its own guard. Closing the host's pipe, including on SIGKILL,
wakes the guard to terminate its group. The launched add-on inherits only null stdio.

## Local backend verification
From the checkout root with Rust 1.99.1, set HOME and TMPDIR to dedicated test
directories. Build the host workspace and the SDK test programs and run the lifecycle
test. Pass the optional Agent package to include a real stdio MCP call to get_state.
The reply's capture data in this test comes from the CLI fixture.

```sh
cargo build --locked -p backtracking-addon-host
cargo build --locked --manifest-path crates/backtracking-addon/Cargo.toml --features install --examples --bins
bash addons/backtracking-agent/pack-macos.sh
python3 crates/backtracking-addon-host/tests/lifecycle.py --agent addons/backtracking-agent/target/backtracking-agent.btaddon
```

bt-addon-cli takes an absolute data root followed by optional secondary. It accepts
one command object per line, fills schema=1, and polls tickets to completion. Send
{"op":"snapshot"} to read current state. Inspect and install require a package path.
Use the review_id returned by inspect in install. A fresh install needs set_enabled
or start. The lifecycle test demonstrates the complete sequence.

bt-seed-profile takes a new absolute root under an existing test folder. The backend
seed uses BACKTRACKING_TEST_ADDONS (tone, tone-opens, quits, broken, atmos, theme,
control, none), BACKTRACKING_TEST_FORMAT, BACKTRACKING_TEST_ADDON_PACKAGE,
BACKTRACKING_TEST_ATMOS_PACKAGE, BACKTRACKING_TEST_THEME_PACKAGE, BACKTRACKING_TEST_ATMOS_OBJECTS,
BACKTRACKING_TEST_EXTRA_PACKAGES and BACKTRACKING_TEST_ALLOW_CONTROL. It stores package
preferences, including the tone format, in the host's normal state file. Application
lane selection, output locations and application preferences belong to the app test profile.

bt-test-sender takes stereo, 7.1.4 or spectrum. Set BACKTRACKING_TEST_SENDER_SECONDS
(default 40), BACKTRACKING_TEST_LANE_B=sender-7.1.4-unlabelled for an unlabelled
stream, BACKTRACKING_TEST_WIDE_TONES=spread or BACKTRACKING_TEST_SPECTRUM_SIDES=split
for the respective variants. bt-check-audio takes a Sender id and checks a non-silent
packet. On macOS use the same isolated HOME for the test app and both tools.

For TEST-06, run bt-test-sender system with BACKTRACKING_TEST_SYSTEM_FORMAT set to
5.1, 7.1 or 7.1.4 followed by :all or :stereo. Sender id c1c1-5e7d-0006 publishes
480-frame packets at 48 kHz, with the matching speaker labels. The test app provides
capture pause/resume and quiet-section retention.
