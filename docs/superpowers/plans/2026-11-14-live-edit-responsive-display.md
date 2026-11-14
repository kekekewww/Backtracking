# Live Editing and Responsive Waveform Implementation Plan

> **For agentic workers:** Use subagent-driven development with one owner per platform UI and a separate shared-policy owner. Track independent tasks below.

**Goal:** Keep recording during selection, remove UI waits on capture work, and make waveforms, guides, and Options readable at different sizes.

**Architecture:** Recording and an immutable editing view have separate lifetimes. Audio capture continues while a background job prepares the editing snapshot; UI reads never wait for capture work. Shared display and snapping policies determine both the drawn guides and the guides eligible for snapping.

**Tech Stack:** Rust core, Windows egui/eframe, VST3/CLAP egui editor, macOS AppKit/Swift through the existing C ABI.

**Spec:** The user confirmed fixed selection-time view/material while background capture continues. Editing and capture controls remain independent; Play preserves the editing material/selection, and existing auto-resume rules remain separate.

## Global Constraints

- Capture must not pause or restart because selection, handle adjustment, scrolling, or export preparation begins or ends.
- Explicit transport Pause remains separate from editing; canceling an edit does not implicitly undo it.
- Editing material retains original absolute sample positions and common A/B boundaries after live history rolls over or is shortened.
- Snap priority is flags, then bars, then seconds; choose the nearest eligible target within each priority class.
- Shift-left-drag directly selects with snapping. Ctrl on Windows / Option on macOS retains free selection; adding Shift also enables snapping.
- Only actually drawn, enabled, visible guides participate. A fully transparent background hides guides but leaves waveform and flag interactions available.
- Exclude a moving handle's previous-frame position from snapping to itself; retain its fixed counterpart and accepted flags.
- Display density uses the actual occupied audio rectangle, not merely the full panel width.
- Long visible spans use fewer waveform bins; editing increases detail. Larger displays also increase waveform and guide stroke widths.
- Options remain a vertical categorized list with right-side scrolling, clearer bold heading levels, and larger body text.
- Dropdowns show their actual row count up to ten and scroll above ten, constrained by available host/screen space.
- Put instructions in documentation. UI copy is limited to headings, labels, values, buttons, and necessary status/error/path information; remove instructional helpers and floating gesture tutorials.
- Full-history save completion must not cancel a new selection created after that request started. Selected-range completion applies only to its own edit.
- Preserve existing settings and exported files. Do not alter user-owned `misc/` files.

## Current Verification Record

- Fresh local code checks: shared core 61 passed / 1 ignored plus two example checks; Windows 47 passed / 2 ignored; plug-in 46 passed / 0 ignored. Remote product CI `55baeac` separately passed Windows and macOS jobs. macOS native build/codesign and named update/Swift/core/plugin tests are verified; interactive GUI/Fn/DAW acceptance remains untested.
- Native Windows test build SHA-256: `0D31F1B6CC302D61C6D6CB258828158675DE9FE142727122C3022F99D8F3B279`. This is an intermediate build before the last footer removal and later plug-in full-save worker changes, not the final package hash.
- Isolated profile, synthetic stereo Sender, observed 1500 × 500 window. All seven History entries displayed; 10→600→60→10 second changes stayed operable, without a timed latency measurement.
- After Pause and endpoint dragging created a roughly 19.77-second edit, Play resumed capture while picture/flags stayed fixed and recording/meter indicators updated. A live-history reduction to ten seconds plus waiting beyond ten seconds retained the edit. Alt+grave saved 948,868 frames at 48 kHz (19.768083333333333 seconds), stereo float32, all finite, and returned live.
- The first baseline WAV was 39.98 seconds while history was still filling; it is not a verified sixty-second export. Source-sequence continuity, native dual-source A/B, and audible artifacts were not verified.
- Native zoom changed the range/guide density on the functional test build. Physical Shift-drag was not performed because the automation cannot hold a modifier through dragging; RawInput/shared tests cover modifier behavior separately.
- Final desktop SHA-256: `5A4A20484991E8C85249BAAAC22D2F453C4B5E886BC24A17A8029C7C4EA2390A`; plug-in DLL: `ED28229A16714BB2C18243FD5F4A2AFA2E51624920D376A300EAF9C4F3789866`. The final desktop was checked natively with an empty footer/no helper tutorials, operable 900×300→700×230 resizing, and visible waveform/bar/seconds rendering. Synthetic fixtures stopped; the final desktop was restarted with the existing user profile and clean Options observed.
- Final Setup SHA-256: `A65B99A0B5C2A1502A3F58EB164CE75AD3D51AA36AE62857D6967A4A3E6F72B7`, 3,486,718 bytes. Desktop/VST3/CLAP and the frozen guide payload bytes match their source files. Installer logic was unchanged; the earlier 17 compile scenarios/19 installation cases were not rerun for this payload.
- Public metadata is under `docs/benchmarks/2026-11-14-responsive-editing/`; audio/private screenshots are excluded. Historical Windows hardware data is unchanged. macOS GUI/physical Fn, DAW hosts, and physical Shift-drag remain outside observed runtime coverage.
- CI run `43537657689` (`55baeac`) failed overall because Linux X11 registration verification failed, despite Windows/macOS success. Retry `43538353959` (`f3d9b0a`) failed at the same step and skipped Windows/macOS. These failures remain historical records, not retroactively green runs.
- Diagnostic retry `43539038043` (`8b2e68b`) failed but confirmed TLDE/LWIN/RWIN and Mod4 plus Super `0x40`; the keyboard description was NULL. This excluded missing fixture keys without claiming invalid ByName mask usage.
- Linux-only follow-up `43539505931` (`52af947`) succeeded after active XkbGetMap/Names querying. Physical TLDE lookup, duplicate rejection and the unchanged held-key `count -eq 1` assertion all passed; no hardcoded keycode/symbol fallback was introduced. Windows/macOS were skipped, so their successful source remains `55baeac`. Wayland/interactive hardware-compositor behavior remains untested.

## Review Focus

- A cold or busy capture worker must not freeze UI metadata or waveform reads.
- Pending snapshots must never cause an edit to export newer live audio by mistake.
- Rolling retention, a duration reduction, and delayed background results must not invalidate editing sample boundaries.
- Manual Pause, clear/source-generation changes, and canceled exports must remain distinct operations.
- Large windows, tiny occupied audio spans, transparent backgrounds, and hidden/dense guides must use consistent drawing and snapping geometry.

## Tasks

### 1. Reproduce and remove capture-lock waits

**Files:** `desktop/windows/src/controller.rs`; diagnostic artifacts under `target/retention-perf-probe/`.

- [x] Trace duration changes, capture locks, peak rendering, and snapshot copies.
- [x] Measure cold/warm synthetic 48/96 kHz and A/B retention workloads; distinguish observed lock contention from an unproven paging hypothesis.
- [x] Demonstrate the old paint-read path failing a deliberately held-lock regression.
- [x] Add cached metadata, `try_render_waveforms(...) -> Option<...>`, and nonblocking `request_snapshots(...) -> Receiver<...>`.
- [x] Verify busy rendering preserves both existing traces and metadata; asynchronous snapshots retain exact sample positions.
- [x] Complete Windows UI integration and rerun its full test suite; record bounded native observations separately.

### 2. Share visible-guide and adaptive display rules

**Files:** `crates/backtracking-core/src/waveform.rs`, shared C ABI and header.

**Interfaces:** `DisplayPolicy::for_view(...)`, `timeline_guides(...)`, `snap_to_visible_markers(...)`; waveform-bin cap 4096 and guide-count cap 2048.

- [x] Reproduce missing priority/seconds/adaptive-density behavior with failing tests.
- [x] Implement bounded, deterministic policies for invalid geometry, extreme frame ranges, zoom, and interaction detail.
- [x] Verify flags beat a nearer bar, bars beat seconds, hidden guides do not snap, and same-class ties choose the earlier frame.
- [x] Integrate shared rules into Windows/plugin and Mac source/C ABI; record successful native macOS CI builds/tests separately from untested GUI/Fn/DAW behavior.

### 3. Continue capture during fixed-material editing

**Files:** Windows `ui.rs`, plugin `editor.rs`/`session.rs`, macOS `BacktrackingApp.swift`/`SystemAudioTap.swift`.

- [x] Implement independent snapshot preparation without stopping capture; verify through code regressions and specified Windows observations.
- [x] Preserve the picture while preparing and publish complete results from the current source generation.
- [x] Move cached editing-peak work away from synchronous UI/capture waits.
- [x] Run continued-input, selection, rollover/shrink, A/B-range, pending/canceled-work and Pause regressions; limit native claims to the recorded single-Sender run.
- [x] Keep cancel/ordinary-click live restoration separate from capture state; preserve later selections when an earlier full-save completes.

### 4. Improve display and Options readability

- [x] Integrate adaptive waveform-bin counts/scalable strokes and shared drawn-guide/snapping policies.
- [x] Increase Options body text and bold hierarchy, remove instructional UI copy, and retain controls/necessary state.
- [x] Run popup/layout regressions and record the native seven-choice Windows popup; do not infer native Mac/DAW popup acceptance.

### 5. Verify, document, and package

- [x] Record fresh shared-core unit/example checks (61 passed / 1 ignored; two examples checked).
- [x] Record fresh Windows test suite (47 passed / 2 ignored).
- [x] Record fresh plug-in test suite (46 passed / 0 ignored).
- [x] Build final Windows/plugin Release artifacts; record native endpoint editing, zoom, resizing and duration switching plus separate automated snapping coverage.
- [x] Record successful native macOS CI build/codesign/update/Swift/core/plugin steps; leave interactive Mac GUI/Fn and DAW acceptance explicitly unverified.
- [x] Update user-facing instructions and record diagnostics/native observations separately without rewriting historical benchmark evidence.
- [x] Rebuild Setup after freezing the guide; verify size/SHA-256 and source-byte payload equality. Keep fresh install/uninstall acceptance separate from this packaging check.

## Explicitly Unverified Runtime Scope

- Physical native Shift-drag, macOS interactive GUI/physical Fn events and permissions, and DAW-host VST3/CLAP acceptance.
- Wayland portal and interactive Linux hardware-keyboard/compositor behavior; successful Linux X11 fixture tests do not imply this coverage.
- Exact source-sequence continuity, native dual-source A/B audio comparison, and audible-artifact absence.
- Full installation/uninstallation of the repackaged payload; prior installer test cases remain historical.

## Visual References

Use the compact audio-tool hierarchy and zoom/readability principles as references, while retaining Backtracking's existing theme and interaction vocabulary:

- [Rolling Sampler update and dynamic zoom](https://www.birdsthings.com/rolling-sampler-update)
- [MiniMeters waveform module](https://minimeters.app/)
