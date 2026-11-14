# Responsive-editing diagnostic evidence

These files are a separate diagnostic dataset based on `58dee34` and a modified
working tree. They do not replace the earlier Windows hardware-monitoring traces.
See [the report](../../RESPONSIVE_EDITING_2026-11-14.md) for interpretation and
integration status.

## Core retention probe

Run from the repository root:

```powershell
cargo run --release --locked -p backtracking-core --example retention_probe
```

The [example source](../../../crates/backtracking-core/examples/retention_probe.rs)
prints 36 JSONL records: 12 stages for 48 kHz stereo, 96 kHz stereo, and
48 kHz stereo A/B. One new process runs the three cases sequentially, constructing
a new Capture for each. Constant `0.05f32` input is batched as quickly as possible
in 10 ms blocks until 600 seconds of audio have been appended. This is not real-time
playback or a GUI/DAW workload.

- `retention-initial.jsonl`: first local-harness observation.
- `retention-initial-source.rs`: the original harness source used for that run.
- `retention-example-rerun.jsonl`: a second observation using the repository example.

The workload/stage counts match, but these are independent observations under
uncontrolled system load and CPU frequency. Preserve the variation instead of
selecting the faster values. Fields `median_ms`, `p95_ms`, and `max_ms` use the
nearest sorted rank at `round((n - 1) * q)`. For even `n=20`, this reported median
is the upper central observation, not the average of two central values. Stages
with `n=1` have only one timing; they do not establish a distribution.

Snapshot timing copies one lane, not a complete A/B WAV export. The
`shrink_180_to_30` stage follows an increase of the limit to 600 seconds without
refilling discarded audio, so only 180 seconds of content remain at that point.
The 1,000 warm expand/shrink pairs use already-shortened content and do not refill
the ten-minute history each time. Fresh process/first writes do not mean cold boot,
cache eviction, paging, HDD, or antivirus emulation.

## Standalone preference-save probe

`synchronous-preference-save.json` records actual included production
`Preferences::save` calls, including directory creation, temporary-file write,
`File::sync_all`, close, atomic rename, and error cleanup. Controller source enums
in the standalone harness are stand-ins only for compiling parsing/default
helpers. The timings cover the complete save call, not `sync_all` in isolation.

The harness writes only under the workspace's `target/preference-perf-probe/` and
reloads the final value there; it does not construct or write the user's real
preference directory. It measures eight first writes in fresh directories, then
64 replacements of the history key in one existing directory. Its quantiles use
linear interpolation (type 7), unlike the core probe's nearest-rank method.

To reconstruct the archived harness from the repository root:

```powershell
$probeRoot = Join-Path (Get-Location) 'target/preference-perf-probe/harness'
New-Item -ItemType Directory -Force -Path (Join-Path $probeRoot 'src') | Out-Null
Copy-Item -LiteralPath 'docs/benchmarks/2026-11-14-responsive-editing/preference-probe-source.rs' -Destination (Join-Path $probeRoot 'src/main.rs')
Copy-Item -LiteralPath 'docs/benchmarks/2026-11-14-responsive-editing/preference-probe-Cargo.toml' -Destination (Join-Path $probeRoot 'Cargo.toml')
Copy-Item -LiteralPath 'docs/benchmarks/2026-11-14-responsive-editing/preference-probe-Cargo.lock' -Destination (Join-Path $probeRoot 'Cargo.lock')
cargo run --release --offline --locked --manifest-path (Join-Path $probeRoot 'Cargo.toml')
```

The harness includes the repository's current `desktop/windows/src/preferences.rs`;
future changes to that implementation may change results. The measured executable
and harness hash are in `preference-probe-provenance.json`. This diagnostic is not
an end-to-end UI-latency or background-writer throughput test.

## Background-writer API follow-up

`asynchronous-preference-save.json` is a separate later run of the production
`PreferenceWriter::new/save/flush` API with the real durable sink. Eight fresh-writer
rounds each enqueue 64 rapid History changes. Constructor, enqueue, enqueue-total,
flush, and drop-after-flush times are separate. All eight successful flushes were
followed by reloading the final History value as 600 seconds with no error poll.

After reconstructing the Cargo manifest/lock above, select the archived asynchronous
harness and its explicit mode:

```powershell
Copy-Item -LiteralPath 'docs/benchmarks/2026-11-14-responsive-editing/preference-async-probe-source.rs' -Destination (Join-Path $probeRoot 'src/main.rs')
cargo run --release --offline --locked --manifest-path (Join-Path $probeRoot 'Cargo.toml') -- --async
```

A successful enqueue means validation/queueing, not persistence, disk latency, or
GUI latency. Only a successful flush plus reload confirms the final saved value;
pending values for the same key may coalesce. This was not a controlled comparison
against the earlier synchronous run, so no speedup ratio is inferred.

The measurement predates later queue-drain/coalescing and review changes. The
production source hash in `preference-probe-provenance.json` is the measured
historical source, not a claim that the final file/binary has that hash or these
latencies. The archived harness preserves the workload; rebuilding uses the
repository's current implementation. The complete historical production source
was not archived, so these files cannot reconstruct the measured binary exactly.
Measured binaries themselves are not published.

## Integrity and limits

`native-editing-observations.json` records the intermediate Windows functional
test build, its native selection/rollover/zoom observations, and two sanitized
WAV metadata records. Generated filenames are removed; no audio is included.
`final-builds.json` separately records final desktop/plugin/Setup hashes, the
copy-cleanup/native-resize observations, and payload byte comparisons after the
guide was frozen. The earlier functional cases are not claimed to have all been
repeated on that final build. Setup packaging integrity is not a fresh
install/uninstall acceptance test.

`evidence-manifest.json` records file hashes and the example source hash. The
diagnostics include no captured audio, user preference values, local absolute
paths, or GPU instance identifiers. The public files do not claim to reproduce
the reported multi-second freeze or identify paging/reboot initialization as its
cause. Final platform tests and UI observations are tracked in the report.
