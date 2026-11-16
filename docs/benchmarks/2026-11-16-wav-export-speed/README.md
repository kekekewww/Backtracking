# WAV export speed evidence: 2026-11-16

This directory times WAV exports of the shared core before and after the export speedup in `crates/backtracking-core/src/export.rs`. The base is commit `19ad609ec82e84137f755f3cf47861e499537366`. The change is that commit with the new `export.rs`, timed before it was committed; `evidence-manifest.json` identifies both versions of the file by Git blob and SHA-256. Both builds ran on one Windows laptop in one sitting, through the same harness, alternating which build went first. Earlier evidence directories and the historical Linux export figures in [`HARDWARE_USAGE.md`](../../HARDWARE_USAGE.md) are unchanged.

## What changed

Long clips were slow to export because of resampling, not because of the disk. Each output frame of a resampled export sums 49 filter taps when the rate goes up and 57 when 48 kHz goes down to 44.1 kHz. Before the change every tap was clamped to the clip and bounds-checked, on one thread, and the two lanes of an A/B pair were written one after the other. The change:

1. Reads interior filter taps straight from the source window. Only frames whose taps reach past an edge of the clip take the clamped path. Both paths sum in the same order, so every output sample keeps its bits.
2. Writes the two files of an A/B pair at the same time, lane B on a second thread. Both file names are claimed first, and a failure in either lane removes both files.
3. Splits each lane's resampling into pieces of 4,096 output frames, renders them on worker threads and writes them in order. An export uses half of the logical processors, at most eight, and a pair gives each lane half of that. On this machine, with 32 logical processors, one lane resamples on 8 threads and each lane of a pair on 4.

Exports at the capture rate do not resample, so only change 2 applies to them. The code that writes a single lane at the capture rate is unchanged. The output did not change: the tests in `local-validation.json` compare streamed files with the whole-clip reference writer on 1, 2, 3 and 8 threads, and the base and changed builds wrote byte-identical files in all 540 exports of the [output check](#output-check).

## Method

- [`export-bench-source.rs`](export-bench-source.rs) is the harness. It was built twice from [`export-bench-Cargo.toml`](export-bench-Cargo.toml) and [`export-bench-Cargo.lock`](export-bench-Cargo.lock), once against each core, with Cargo's default release profile. The desktop app's release profile differs from it only by stripping symbols.
- Each case holds two stereo lanes of deterministic noise in a `Capture` with a 10-minute history that keeps quiet sections, pushed in 10 ms blocks and then frozen.
- Each timed run passes the frozen tracks to `export_routed_tracks_with_profile`, as the Windows app does when it writes a clip, with 32-bit float output and merged routes. The plug-in and the macOS app reach the same code through `export_routed_tracks_with_tempo` and `export_routed_track_refs`. A run covers claiming the file names, resampling, encoding, writing and closing the files. Building and freezing the capture, reading the file sizes and deleting the files are not timed.
- Files went to a temporary folder on the C: volume, the volume that holds the default export folder: an NVMe SSD formatted NTFS. Microsoft Defender real-time protection was on.
- Eight harness processes alternated the builds: base then change for 60 s at 48 kHz, change then base for 60 s at 44.1 kHz, base then change for 180 s, and change then base for 600 s. In each process, every export rate ran its pair runs and then its lane-A runs. Cases have five runs, 600-second cases three.
- Plain writes of new files the size of one lane's audio at 96 kHz for 60, 180 and 600 seconds followed the exports, in 4 KiB pieces through an 8 KiB `BufWriter` as the exporter writes, and in one call.
- The whole sequence ran twice. The first pass ran every run straight after the one before. In it, some runs of the change and some plain writes took several times as long as the fastest run of the same case, while the base's resampled runs stayed within 1.37 times. The second pass repeated everything with a 3-second untimed pause before each run, to let the disk work of earlier runs settle. That is also closer to dragging one clip at a time. The harness gained its optional pause argument between the passes, and `evidence-manifest.json` gives the source hash each pass ran. Both passes are published and summarized separately.
- Every run is a line of the pass's `export-timings-*.jsonl` or `disk-writes-*.jsonl` file, after an `invocation` line from the runner that gives each process's order and start time. [`summarize-timings.py`](summarize-timings.py) reports each case's median, minimum and maximum over all of its runs into [`summary.json`](summary.json). Nothing is warmed up, averaged or discarded. The script also checks that both builds wrote the same number of bytes.

## Results

Times are seconds per export: the median, with the fastest and slowest run in brackets. Speedup is the base median divided by the change median.

### Second pass: 3-second pause before each run

A/B pair, two stereo files:

| Clip | Source → export | Runs | Base | Change | Speedup |
| ---: | --- | ---: | ---: | ---: | ---: |
| 60 s | 48 kHz → 96 kHz | 5 | 1.281 (1.242–1.332) | 0.103 (0.099–0.112) | 12.4× |
| 60 s | 48 kHz → 48 kHz | 5 | 0.083 (0.076–0.129) | 0.051 (0.050–0.055) | 1.6× |
| 60 s | 48 kHz → 44.1 kHz | 5 | 0.662 (0.643–0.692) | 0.053 (0.048–0.056) | 12.6× |
| 60 s | 44.1 kHz → 96 kHz | 5 | 1.231 (1.224–1.251) | 0.113 (0.089–0.172) | 10.9× |
| 60 s | 44.1 kHz → 48 kHz | 5 | 0.623 (0.606–0.637) | 0.049 (0.048–0.052) | 12.7× |
| 180 s | 48 kHz → 96 kHz | 5 | 3.699 (3.560–3.739) | 0.289 (0.272–0.294) | 12.8× |
| 180 s | 48 kHz → 48 kHz | 5 | 0.221 (0.189–0.225) | 0.118 (0.116–0.140) | 1.9× |
| 180 s | 48 kHz → 44.1 kHz | 5 | 1.917 (1.888–2.031) | 0.150 (0.143–0.173) | 12.8× |
| 600 s | 48 kHz → 96 kHz | 3 | 13.157 (12.794–13.483) | 1.300 (1.229–1.364) | 10.1× |
| 600 s | 48 kHz → 48 kHz | 3 | 0.777 (0.751–0.799) | 0.460 (0.453–0.462) | 1.7× |

Lane A alone, one stereo file:

| Clip | Source → export | Runs | Base | Change | Speedup |
| ---: | --- | ---: | ---: | ---: | ---: |
| 60 s | 48 kHz → 96 kHz | 5 | 0.619 (0.592–0.680) | 0.079 (0.070–0.081) | 7.8× |
| 60 s | 48 kHz → 48 kHz | 5 | 0.042 (0.040–0.051) | 0.044 (0.041–0.049) | 0.9× |
| 60 s | 48 kHz → 44.1 kHz | 5 | 0.324 (0.316–0.332) | 0.036 (0.033–0.040) | 9.1× |
| 60 s | 44.1 kHz → 96 kHz | 5 | 0.616 (0.589–0.685) | 0.078 (0.072–0.092) | 7.9× |
| 60 s | 44.1 kHz → 48 kHz | 5 | 0.311 (0.309–0.318) | 0.037 (0.034–0.038) | 8.4× |
| 180 s | 48 kHz → 96 kHz | 5 | 1.833 (1.812–1.881) | 0.233 (0.202–0.241) | 7.9× |
| 180 s | 48 kHz → 48 kHz | 5 | 0.117 (0.103–0.132) | 0.109 (0.104–0.122) | 1.1× |
| 180 s | 48 kHz → 44.1 kHz | 5 | 0.981 (0.940–1.009) | 0.105 (0.096–0.114) | 9.4× |
| 600 s | 48 kHz → 96 kHz | 3 | 6.732 (6.580–6.806) | 0.923 (0.773–1.023) | 7.3× |
| 600 s | 48 kHz → 48 kHz | 3 | 0.377 (0.336–0.422) | 0.374 (0.352–0.376) | 1.0× |

Plain writes of new files:

| Bytes | Pattern | Runs | Time | MB/s at median |
| ---: | --- | ---: | ---: | ---: |
| 46,080,000 | 4 KiB pieces, 8 KiB `BufWriter` | 5 | 0.045 (0.039–0.049) | 1,035 |
| 46,080,000 | one call | 5 | 0.037 (0.031–0.039) | 1,232 |
| 138,240,000 | 4 KiB pieces, 8 KiB `BufWriter` | 5 | 0.125 (0.108–0.133) | 1,109 |
| 138,240,000 | one call | 5 | 0.109 (0.107–0.111) | 1,274 |
| 460,800,000 | 4 KiB pieces, 8 KiB `BufWriter` | 3 | 0.394 (0.371–0.395) | 1,169 |
| 460,800,000 | one call | 3 | 0.371 (0.339–0.371) | 1,242 |

Resampled exports were 10.1 to 12.8 times as fast for pairs and 7.3 to 9.4 times for one lane. Pairs gain more because their two lanes, which used to run one after the other, now run at the same time. At the capture rate, pairs were 1.6 to 1.9 times as fast because both files are written at once. Single lanes at the capture rate, whose code did not change, measured 0.9 to 1.1 times the base.

Before the change, a 600-second lane at 96 kHz took 6.732 s, and a plain write of the same 460.8 MB took 0.394 s, about 6% of that. After the change the export took 0.923 s, and the same plain write amounts to 43% of it. The disk is now a large part of a long export.

### First pass: runs back to back

A/B pair, two stereo files:

| Clip | Source → export | Runs | Base | Change | Speedup |
| ---: | --- | ---: | ---: | ---: | ---: |
| 60 s | 48 kHz → 96 kHz | 5 | 1.378 (1.234–1.431) | 0.175 (0.152–0.208) | 7.9× |
| 60 s | 48 kHz → 48 kHz | 5 | 0.088 (0.079–0.119) | 0.068 (0.040–0.245) | 1.3× |
| 60 s | 48 kHz → 44.1 kHz | 5 | 0.722 (0.701–0.772) | 0.090 (0.056–0.321) | 8.1× |
| 60 s | 44.1 kHz → 96 kHz | 5 | 1.582 (1.498–1.818) | 0.131 (0.111–0.485) | 12.1× |
| 60 s | 44.1 kHz → 48 kHz | 5 | 0.801 (0.684–0.865) | 0.060 (0.054–0.100) | 13.4× |
| 180 s | 48 kHz → 96 kHz | 5 | 4.166 (4.044–4.407) | 0.803 (0.780–0.855) | 5.2× |
| 180 s | 48 kHz → 48 kHz | 5 | 0.258 (0.248–0.635) | 0.213 (0.129–0.628) | 1.2× |
| 180 s | 48 kHz → 44.1 kHz | 5 | 2.287 (2.110–2.511) | 0.181 (0.168–0.626) | 12.6× |
| 600 s | 48 kHz → 96 kHz | 3 | 13.768 (13.670–14.501) | 2.533 (2.415–2.541) | 5.4× |
| 600 s | 48 kHz → 48 kHz | 3 | 1.122 (0.929–1.249) | 0.540 (0.412–0.803) | 2.1× |

Lane A alone, one stereo file:

| Clip | Source → export | Runs | Base | Change | Speedup |
| ---: | --- | ---: | ---: | ---: | ---: |
| 60 s | 48 kHz → 96 kHz | 5 | 0.679 (0.660–0.798) | 0.089 (0.074–0.209) | 7.7× |
| 60 s | 48 kHz → 48 kHz | 5 | 0.050 (0.040–0.064) | 0.038 (0.035–0.039) | 1.3× |
| 60 s | 48 kHz → 44.1 kHz | 5 | 0.344 (0.330–0.424) | 0.045 (0.044–0.052) | 7.6× |
| 60 s | 44.1 kHz → 96 kHz | 5 | 0.896 (0.788–1.081) | 0.111 (0.082–0.211) | 8.1× |
| 60 s | 44.1 kHz → 48 kHz | 5 | 0.361 (0.349–0.398) | 0.044 (0.033–0.321) | 8.3× |
| 180 s | 48 kHz → 96 kHz | 5 | 2.043 (2.038–2.570) | 0.629 (0.237–0.698) | 3.2× |
| 180 s | 48 kHz → 48 kHz | 5 | 0.122 (0.115–0.194) | 0.186 (0.109–0.246) | 0.7× |
| 180 s | 48 kHz → 44.1 kHz | 5 | 1.188 (1.059–1.310) | 0.153 (0.108–0.521) | 7.7× |
| 600 s | 48 kHz → 96 kHz | 3 | 6.946 (6.910–7.609) | 1.687 (1.185–1.771) | 4.1× |
| 600 s | 48 kHz → 48 kHz | 3 | 0.597 (0.387–0.652) | 0.477 (0.421–0.481) | 1.2× |

Plain writes of new files:

| Bytes | Pattern | Runs | Time | MB/s at median |
| ---: | --- | ---: | ---: | ---: |
| 46,080,000 | 4 KiB pieces, 8 KiB `BufWriter` | 5 | 0.059 (0.049–0.099) | 780 |
| 46,080,000 | one call | 5 | 0.048 (0.031–0.080) | 965 |
| 138,240,000 | 4 KiB pieces, 8 KiB `BufWriter` | 5 | 0.159 (0.108–0.505) | 868 |
| 138,240,000 | one call | 5 | 0.174 (0.095–1.500) | 793 |
| 460,800,000 | 4 KiB pieces, 8 KiB `BufWriter` | 3 | 0.667 (0.581–0.792) | 691 |
| 460,800,000 | one call | 3 | 1.875 (0.774–1.973) | 246 |

In the first pass, the slowest run of a case took at most 1.37 times its fastest for the base's resampled exports, but up to 9.7 times for the change and 15.8 times for a plain write. With the pause, those figures fell to 1.16, 1.94 and 1.28. First-pass medians were up to 1.58 times the second pass's for the base and up to 2.78 times for the change. The change's single lane at the capture rate, whose code did not change, measured 0.7 to 1.3 times the base in the first pass. These runs show how much back-to-back exports on this nearly full volume can vary. They do not show that the pause removed every effect of earlier runs.

## Tests

The four test commands of the Windows CI job passed on the changed source with the local toolchain. The core library had 109 passed and 1 ignored, the TCP-loopback test. The shared interface had 27 passed. The Windows desktop had 109 passed and 3 ignored: the native Process Loopback smoke, the durable preference-write probe and the real Alt+grave shortcut test. The VST3/CLAP plug-in had 71 passed. The plug-in build printed one dead-code warning for three setters in `plugins/backtracking-plugin/src/session.rs`, a file this change does not touch.

[`local-validation.json`](local-validation.json) lists the commands and the four export tests that cover this change. They check that streamed files are byte-identical to the whole-clip writer on 1, 2, 3 and 8 threads, and that the unclamped interior sums have the same bits as the clamped path. They also check that a lane failing part way leaves no file, alone or in a pair, and that a failed write leaves no resampling thread waiting.

## Output check

[`output-check-source.rs`](output-check-source.rs) was built as a second binary of the harness package, once against each core. It ran on 2026-11-16 at 07:39 UTC, and again at 07:42 UTC after a change to how `compare-outputs.py` reads its input; both runs gave the same comparison. It exports two lanes of deterministic noise through `export_routed_tracks_with_profile` in 540 combinations:

- 48 and 44.1 kHz sources, exported at 96, 48 and 44.1 kHz;
- 16-bit, 24-bit and 32-bit float samples;
- stereo lanes merged and forced to mono, and mono lanes placed left and right, as side and merged, and as mid and center, which between them use every channel map;
- the whole 20-second clip and two short ranges that start part way in;
- the A/B pair and lane A alone.

Each build wrote 810 files, 1,580,053,758 bytes. [`compare-outputs.py`](compare-outputs.py) hashed every file of both builds: all 810 are byte-identical, including the 540 resampled ones. [`output-comparison.jsonl`](output-comparison.jsonl) lists each file's case, size and SHA-256; the WAV files themselves are not published. A copy of one file with a single bit flipped hashed differently, so the comparison can fail.

## Reproduce

From the repository root of a checkout with this change, in PowerShell:

```powershell
$evidence = 'docs/benchmarks/2026-11-16-wav-export-speed'
$bench = 'target/wav-export-bench'
New-Item -ItemType Directory -Force "$bench/src" | Out-Null
Copy-Item "$evidence/export-bench-Cargo.toml" "$bench/Cargo.toml"
Copy-Item "$evidence/export-bench-Cargo.lock" "$bench/Cargo.lock"
Copy-Item "$evidence/export-bench-source.rs" "$bench/src/main.rs"
cargo build --release --locked --manifest-path "$bench/Cargo.toml"
& "$bench/target/release/wav-export-bench.exe" exports <empty-folder> change 60 48000 96000,48000,44100 5
```

For the base, add a worktree of the base commit with `git worktree add <folder> 19ad609`, run the same commands in it with the harness files copied from this checkout, and pass `base` as the label. The other cases change the clip length, source rate, export rates and run count. `wav-export-bench.exe writes <empty-folder> 46080000,138240000 5` times the plain writes. For the second pass, add `3000` after the run count of either mode. Collect each process's lines, as UTF-8, in the pass's `export-timings-*.jsonl` or `disk-writes-*.jsonl` file, for example with `| Out-File -Append -Encoding utf8 <file>`, then run `python summarize-timings.py` in this folder to rewrite `summary.json` and print the tables.

For the output check, copy `output-check-source.rs` to `target/wav-export-bench/src/bin/output-check.rs` in each checkout and build again. Run `output-check.exe <empty-folder>` from each build's `target/release`, saving its lines as UTF-8, then `python compare-outputs.py <base-folder> <base-lines> <change-folder> <change-lines>` to rewrite `output-comparison.jsonl`. Each folder takes about 1.6 GB.

## Limitations

- One laptop with 32 logical processors, on AC power with the Turbo power scheme. CPU clocks, temperature, other running applications, the Windows file cache and the SSD's own caching were not controlled.
- Machines with fewer logical processors resample on fewer threads, one per lane with 2 or 3 logical processors. None was measured.
- The harness calls the shared core's export function. No export was timed in the desktop app, the plug-in or a DAW, and no drag was timed.
- macOS was not built or timed locally. The Swift app reaches the same core through `export_routed_track_refs`.
- Only 32-bit float output was timed. 16- and 24-bit PCM share the resampler but were not timed.
- A run ends when its files are closed. Neither the exporter nor the harness flushes them to the device, so Windows may still be writing cached data after a run.
- The C: volume had about 9.2 GB free, about 0.9%, during the runs, and Microsoft Defender real-time protection was on.
- The audio was synthetic noise. The resampler does the same work whatever the samples are.
- The output check compares the two builds with each other on synthetic noise in 540 combinations. Other lengths, offsets and content are covered only by the unit tests.
- The harness was built with rustc 1.95.0. CI builds with 1.99.1.
- Raw test logs are not published. `local-validation.json` copies the counts from the `cargo test` summaries.

## Files

- [`export-timings-paused.jsonl`](export-timings-paused.jsonl) and [`export-timings-back-to-back.jsonl`](export-timings-back-to-back.jsonl): every export run of both builds, in the second and the first pass.
- [`disk-writes-paused.jsonl`](disk-writes-paused.jsonl) and [`disk-writes-back-to-back.jsonl`](disk-writes-back-to-back.jsonl): every plain write, in the second and the first pass.
- [`summary.json`](summary.json): medians and ranges of both passes, computed by [`summarize-timings.py`](summarize-timings.py).
- [`local-validation.json`](local-validation.json): test-suite results for the changed source, and the output check's summary.
- [`output-comparison.jsonl`](output-comparison.jsonl): every file of the output check, with the SHA-256 its copies from both builds share, written by [`compare-outputs.py`](compare-outputs.py).
- [`hardware.json`](hardware.json): machine, storage, toolchain and power state.
- [`export-bench-source.rs`](export-bench-source.rs), [`export-bench-Cargo.toml`](export-bench-Cargo.toml), [`export-bench-Cargo.lock`](export-bench-Cargo.lock): the harness.
- [`output-check-source.rs`](output-check-source.rs): the output check, built with the harness's Cargo files.
- [`evidence-manifest.json`](evidence-manifest.json): sizes and SHA-256 of these files, the source versions and the harness builds.
