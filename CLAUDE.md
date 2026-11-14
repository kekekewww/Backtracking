# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Backtracking is a retrospective audio sampler: it keeps the last 10 s–10 min of audio in memory and lets the user save or drag a range out as WAV. It ships as a Windows desktop app, a macOS desktop app, a VST3/CLAP plug-in, and a Linux global-shortcut helper, all built on one shared Rust core.

## Build layout: four separate Cargo roots

The root workspace contains **only** `crates/backtracking-core`. The other Rust crates are excluded from it and each has its own `Cargo.lock` and `target/`, so root-level `cargo` commands do not touch them. Always pass `--manifest-path` (and `--locked`) for those.

| Component | Manifest | Notes |
| --- | --- | --- |
| Core | `Cargo.toml` (root workspace) | edition 2021, MSRV 1.82, no dependencies, builds `rlib` + `staticlib` |
| Windows desktop | `desktop/windows/Cargo.toml` | edition 2024, MSRV 1.95, binary `Backtracking`; all modules are `cfg(target_os = "windows")` |
| Plug-in | `plugins/backtracking-plugin/Cargo.toml` | edition 2024, MSRV 1.95, `cdylib`; `editor` feature is default |
| Linux shortcut helper | `desktop/linux/Cargo.toml` | Rust + `src/hotkey.c` (X11 / GLib portal); needs `libx11-dev libglib2.0-dev` |
| macOS desktop | `desktop/macos/build.sh` | Swift, not Cargo; links `libbacktracking_core.a` via `crates/backtracking-core/include/backtracking.h` |

CI (`.github/workflows/desktop-verification.yml`) pins Rust 1.99.1 and runs on Windows, macOS and Linux for PRs touching `Cargo.*`, `crates/`, `desktop/`, `plugins/`.

## Commands

```sh
# Core (root workspace)
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings

# Windows desktop and plug-in
cargo test --locked --manifest-path desktop/windows/Cargo.toml
cargo test --locked --manifest-path plugins/backtracking-plugin/Cargo.toml
cargo build --locked --release --manifest-path desktop/windows/Cargo.toml   # -> desktop/windows/target/release/Backtracking.exe

# Linux helper
cargo test --locked --manifest-path desktop/linux/Cargo.toml

# One test by name; add `-- --ignored` for opt-in tests
cargo test --locked -p backtracking-core <test_name>
cargo test --locked --manifest-path desktop/windows/Cargo.toml <test_name>
cargo test --locked --workspace same_named_senders_fan_out_and_snapshot_targets_are_independent -- --ignored
```

`#[ignore]` tests touch real system state (TCP loopback, the real Alt+grave hotkey, native Process Loopback, durable preference writes); run them deliberately, not as a blanket `--ignored`.

Plug-in bundles (VST3 + CLAP land in `target/bundled/`; needs `cargo install cargo-nice-plug --locked`):

```powershell
$env:CARGO_TARGET_DIR = Join-Path (Get-Location) target
cargo nice-plug bundle backtracking-plugin --release --locked --manifest-path plugins/backtracking-plugin/Cargo.toml
```

macOS only: `bash desktop/macos/build.sh` (app), `bash desktop/macos/verify-capture.sh` (Swift capture-routing / paired-WAV harness, compiled with `-D BACKTRACKING_CAPTURE_VERIFY`), `bash plugins/backtracking-plugin/bundle-macos.sh` (plug-ins). The updater tests compile `StartupUpdater.swift` + `tests/StartupUpdaterTests.swift` with `-D BACKTRACKING_UPDATER_TESTS`; see the workflow for the exact `swiftc` line.

Windows installer: `powershell -NoProfile -File tools/build_windows_setup.ps1` (NSIS; output in `target/setup/`), tests via `tools/test_windows_setup.ps1`. Details in `desktop/windows/installer/README.md`.

Release builds embed the update public key only when `BACKTRACKING_UPDATE_PUBLIC_KEY` is set to the contents of `desktop/update-public-key.hex` (64 lowercase hex chars; both build scripts validate it). Release signing and artifact naming are in `docs/UPDATES.md`.

## Architecture

### Core (`crates/backtracking-core`)

Everything that must behave identically across front ends lives here; the front ends own only OS audio capture, windows, and persistence.

- `capture` (+ `capture/history`) — `Capture`: the retained-history ring, silence compression vs. "keep quiet sections", pre-roll/tail/join fades, and `Snapshot` (immutable material used for editing and export).
- `synchronized` — `SynchronizedCapture`: aligns two timestamped lanes (desktop A/B sources) on one clock with jitter/drift/gap handling, plus `ChannelRoute` (Left/Right/Center/Mid/Side/Merged).
- `queue` — lock-free hand-off from the audio producer thread (`AudioQueue`, `TimedAudioQueue`).
- `export` — WAV writing, resampling to 44.1/48/96 kHz, 16/24-bit PCM or 32-bit float, single and paired A/B track export.
- `waveform` (+ `waveform/geometry`) — display policy, min/max envelope vs. per-sample polyline geometry, timeline guides, flag/bar/second snapping, minimum zoom. UI drawing code in each front end should consume these rather than reimplement them.
- `meter` — dBFS peak, VU, momentary LUFS.
- `sender` — local IPC between plug-in instances and desktop/helper processes: each `SenderEndpoint` listens on a localhost TCP port and advertises itself with a `<id>.sender` registration file under a per-user `Backtracking/senders-v1` directory; `discover_senders`, `SenderReceiver` and `request_sender_snapshot` are the client side.
- `ffi`, `track_ffi` — the C ABI (`BtCore` single source, `BtTracks` A/B) consumed by the Swift app.

**Changing a core API usually means changing three consumers:** the Rust callers (Windows desktop, plug-in), the `extern "C"` wrappers in `ffi.rs`/`track_ffi.rs`, and the hand-maintained `include/backtracking.h` plus its Swift call sites. The header is not generated. Its comments also state the threading contract (controls and `bt_poll` on one thread, exactly one audio producer).

### Front ends

- **Windows desktop** (`desktop/windows/src`) — `loopback` does WASAPI capture (system, microphone, per-process) and feeds timed packets to `controller`, which owns the capture worker and `SynchronizedCapture`; `ui` is the eframe/egui window (waveform, Options, export jobs); `preferences` persists settings with a background writer; `updater` + `update_protocol` implement signed (Ed25519) self-update; `tray`, `snapshot_hotkey`, `startup`, `file_drag`, `window_order` are Win32 integration. `main.rs` runs the update helper / single-instance check before starting the UI; `--new-capture-window` opens a secondary recorder.
- **Plug-in** (`plugins/backtracking-plugin/src`) — `lib.rs` is the nice-plug `Plugin` (pass-through audio, optional sidechain); `session` holds shared state plus the background `Worker` and Sender worker so nothing blocking runs on the audio thread; `settings` is the per-instance state persisted into the DAW project under the `capture_settings_v1` key; `editor` is the egui UI with platform drag-out (`editor/win_drag`, `mac_drag`).
- **macOS desktop** (`desktop/macos`) — `SystemAudioTap.swift` (Core Audio taps, routing, the verify harness), `BacktrackingApp.swift` (AppKit UI), `StartupUpdater.swift`.
- **Linux** (`desktop/linux`) — no recorder; a helper that registers Super+grave and sends a snapshot request to a plug-in instance through the Sender mechanism.

The Windows desktop UI and the plug-in editor are separate egui implementations of the same interface, and the macOS app is a third in Swift. A user-visible behavior change generally has to be made in all three, with shared logic pushed into the core.

`plugins/backtracking-plugin/vendor/nice-plug` is a vendored copy of the nice-plug framework (pinned `=0.4.2`), not project code.

## Project conventions

- User-facing docs (`README.md`, `docs/USER_GUIDE.md`, `docs/UPDATES.md`) are bilingual: Traditional Chinese first, then English. Update both languages together.
- Operating instructions belong in the docs; the UI keeps only controls and necessary state. Don't add helper prose, tutorial overlays, or new technical status labels to Options.
- New settings must default so that old desktop preferences and old plug-in project state load unchanged.
- Verification records (`docs/*_2026-*.md`, `docs/benchmarks/`, `docs/superpowers/plans/`) separate what was actually observed from what was not (e.g. macOS GUI, DAW hosting, Wayland) and tie results to a commit hash. Keep that distinction when updating them; a passing suite on one platform is not evidence for another.
- The desktop crates, the plug-in and the Linux helper carry the same version number (currently in each `Cargo.toml`; macOS uses `CFBundleShortVersionString` in `desktop/macos/Info.plist`).
