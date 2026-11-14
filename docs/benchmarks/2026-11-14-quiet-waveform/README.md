# Quiet capture and waveform evidence: 2026-11-14

This new evidence directory belongs to source `5fe9224c3dc54d0a2a655ec6b66522c43bd7a238`, quiet retention and the hybrid waveform round. Earlier `2026-11-14-windows` and `2026-11-14-responsive-editing` evidence remains unchanged. `local-validation.json` records local suites and intermediate Windows GUI observations through 0.01-second wheel zoom. `package-validation.json` records final local Release/Setup hashes, four payload/source byte checks and the desktop-only installed refresh. `cross-platform-ci.json` records completed [run 43549431632](https://github.com/kekekewww/Backtracking/actions/runs/43549431632): Windows, macOS and Linux jobs all succeeded on the same source. The native quiet record below is one scoped case.

`native-quiet-smoke.json` preserves both the initial `STATUS_HEAP_CORRUPTION` failure and the successful repeat after the existing Process Loopback activation ownership fix. The successful test ran in a debug test binary. Its frame counters and observed intervals describe the case; they are not latency or universal audio-quality guarantees. Source-review findings and the 64-iteration ownership regression are identified as reports from the implementation owner, separately from the parsed native output.

Reproduce from the repository root on an eligible Windows system:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File tools/run_native_quiet_smoke.ps1
```

The runner compiles [the std-only silent process fixture](../../../tools/quiet_process_fixture.rs), launches it hidden, verifies its exact child PID, and runs the explicitly opted-in test with a 60-second bound. The test permits only a `backtracking-quiet*` executable under the workspace `target` directory. It uses dedicated Process Loopback with no System Audio fallback. On-phase snapshots remain in memory and are checked as zero. It writes no audio, user preferences, registry or UI; the runner writes only workspace fixture/log/stop-marker outputs. The [runner source](../../../tools/run_native_quiet_smoke.ps1) and fixture hashes appear in `evidence-manifest.json`; tools retain their own source line-ending policy.

The published JSON is a sanitized scalar observation record, not a raw log dump. It whitelists timestamps, classifications, counters, intervals, assertion names and hashes; it removes process IDs, absolute paths, user names and raw error/stdout/stderr text. Hashes identify the private local input logs without publishing them. No private image or WAV is included. The failed attempt recorded a fixture executable hash but no fixture source hash; the latter is deliberately left absent.

The source ownership repair does not establish the cause of previous multi-second UI stalls, paging or startup behavior. This case does not establish macOS/Fn/DAW/real-device coverage, WAV duration, audible artifact freedom, physical input behavior or waveform performance. CLAP live processing requests KeepAlive/CLAP_CONTINUE when `retain_silence || sender_enabled`; offline processing and VST3 stay Normal, with VST3 kNoTail. Already sleeping/stopped hosts require a future real callback, and no callback is replaced with fabricated audio.

See the [current-round report](../../QUIET_CAPTURE_AND_WAVEFORM_2026-11-14.md) for design, results and outstanding coverage.
