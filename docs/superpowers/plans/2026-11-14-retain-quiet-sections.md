# Retain quiet sections and continuous waveform display

Add a capture preference, disabled by default, to Windows, macOS, VST3 and CLAP. The only new UI copy is the checkbox label: `Keep quiet sections` / `保留安靜片段`.

This round also implements a shared hybrid waveform renderer: continuous filled min/max envelopes and outlines at wider views, original-sample polylines close up, and a common width-based minimum zoom. Documentation explains operation; the UI keeps controls and necessary state.

**Status:** implementation/local suites, scoped intermediate Windows GUI, final local package/desktop refresh and full Windows/macOS/Linux CI are recorded for source `5fe9224c3dc54d0a2a655ec6b66522c43bd7a238`. [Run 43549431632](https://github.com/kekekewww/Backtracking/actions/runs/43549431632) completed successfully with all three jobs green. Earlier reports remain historical; see the [current-round record](../../QUIET_CAPTURE_AND_WAVEFORM_2026-11-14.md) for remaining runtime limits. The intermediate GUI executable hash remains separate from final packaged artifacts.

- Enabled: retain subsequent quiet and zero-valued input and known source-clock silence; preserve A/B alignment and available host markers.
- Disabled: keep existing silence compression.
- Apply changes on capture workers after pending input is handled; keep retained history, recording state and immutable editing material.
- Do not restore audio already discarded before enabling the option.
- Preserve paused/disconnected-source semantics; do not fabricate unknown host transport information.
- Bound work and storage for long known timestamp gaps. Windows loopback needs periodic idle padding with a progressing clock while this option is enabled.
- A source that is unavailable or a Sender that disconnected is not an invitation to generate unknown audio indefinitely. Windows no-packet padding is scoped to eligible connected capture; macOS without callbacks must wait for real subsequent source timestamps. Do not promise equivalent immediate trailing padding without native evidence.
- Persist desktop preferences and plug-in instance state; old settings/projects default to disabled.
- Toggling does not retroactively remove quiet already retained while On or restore quiet discarded while Off. It does not clear history, change manual Pause, or invalidate pinned editing samples/A-B boundaries.
- Keep all explanations in documentation; do not add helper prose, tutorial overlays or new technical state labels to Options.

## Implementation and validation

- [x] Shared capture, synchronized lanes, bounded silence history, and C ABI; default, transition, exact samples, alignment, and markers regressions in the local suites.
- [x] Windows background settings/controller, Capture checkbox, and loopback idle timing regressions; scoped native opt-in result recorded separately.
- [x] Plug-in settings/state, worker application, checkbox, and independent Sender processing regressions; actual DAW operation remains unverified.
- [x] macOS preferences/checkbox/worker application and native Swift geometry/routing/quiet/alignment/A-B WAV harness in CI; Mac GUI/Fn/DAW remain unverified.
- [x] Shared `waveform_geometry`: Empty, finite-run Envelope with bin edges/centers, and original Samples using `x = i / N` without a half-bin offset.
- [x] Windows/macOS/plug-in filled envelope + outline and original-sample polyline adapters, preserving selection/pinned material/A-B coordinates; platform suites/builds do not imply all GUI runtime coverage.
- [x] Shared occupied-width zoom limit (about four logical pixels per sample), bounded inputs and automated view-preservation checks across resize.
- [x] Geometry extrema/finite-run/zero/DC/impulse and mode/zoom boundary tests; intermediate Windows wheel zoom observed through 0.01 seconds. No new native resize or physical Shift-drag observation.
- [x] Local suites reported by the integrator: core 82 passed/1 ignored plus Timeline Sender example 2 passed; Windows 73 passed/3 ignored; plug-in 64 passed/0 ignored. Native quiet 1 passed is a separate opt-in case.
- [x] Final local Windows desktop/plugin/Setup releases and four-payload byte checks; intermediate Windows manual Pause/wheel 10.00→4.51→0.42→0.04→0.01-second continuous display observation.
- [x] Full current-source CI: Windows tests/Release, macOS build/update rollback/native Swift harness/tests, Linux helper/Sender/X11 checks; all three jobs and run 43549431632 succeeded.
- [x] Intermediate Windows GUI: default checkbox unchecked, On/Off changes, no helper text, Clear followed by full 10.00-second continuous envelope. Keep its runtime hash separate from the final title-cleanup build; no fresh final-GUI interaction claimed.
- [x] Record the first native Process Loopback crash, existing VT_BLOB ownership repair and fixed-case quiet Off/On/Pause/Resume result in a separate [new evidence record](../../benchmarks/2026-11-14-quiet-waveform/native-quiet-smoke.json); do not generalize it to all sources, GUI, DAW or freeze causes.
- [x] Update README/User Guide for default Off, future-only retention, history/pinned-material preservation and source-clock limits.
- [x] Publish the pre-update point/column analysis and this round's adopted hybrid-renderer contract, explicitly distinguishing design from verification.
- [x] Rebuild Setup and refresh the actual installed desktop with backup and verified executable/Guide hashes. Installed plug-ins remain unchanged; no new-payload Setup install/uninstall rerun.

## Adopted waveform display design

The point/column appearance is consistent with the pre-update independent-bin geometry; it does not itself prove missing audio. This round implements continuous min/max envelopes and original-sample lines. Quiet retention and drawing remain separate policies, each requiring validation.

See [Waveform display research](../../WAVEFORM_DISPLAY_RESEARCH_2026-11-14.md) for the two private-image descriptions without images/paths, pre-update min/max/cache behavior, primary official references and adopted mode switching. Existing 128-frame peak caches and raw samples are available; no framework migration is required. The previous Windows 10 ms/macOS 50 ms/plug-in 100 ms zoom floors are replaced by the shared occupied-width policy in this round. Deeper interpolation and RMS overlays remain optional follow-up work.

## Validation matrix and remaining runtime limits

- Default Off and older preference/project loading preserve the existing gate/compression behavior.
- On retains delivered quiet, exact zeros, and only silence established by an eligible source clock; A/B alignment and available marker offsets remain consistent.
- On/Off transitions process already queued input in the intended order and retain history, fixed editing samples, flags and explicit Pause; discarded earlier silence is not restored.
- Large known gaps have bounded work/storage, retain only the allowed live-history tail and do not allocate an unbounded zero buffer.
- Manual Pause appends no silence; unavailable source/disconnected Sender does not synthesize an unbounded stream; missing host timing does not create invented bar positions.
- Windows no-packet idle advancement has a progressing eligible capture clock, does not double-count later packets, and stops correctly on Pause/disconnect/source replacement.
- macOS delivered silence and real timestamp gaps are tested separately from no-callback trailing behavior; the latter remains a native-verification limit until observed.
- CLAP live processing alone returns KeepAlive/CLAP_CONTINUE when `retain_silence || sender_enabled`. Offline processing and VST3 remain Normal (VST3 kNoTail), avoiding an infinite-tail declaration that changes offline export/freeze. Audio RT reads AtomicBool; no new vendor/dummy parameter/restart/wake path is added. Public GuiContext/ProcessContext offers no safe request_process interface, so an already sleeping/stopped host needs its next real callback. No callbacks means no invented audio; actual DAW behavior remains unverified.
- The geometry helper preserves Envelope extrema and separates received non-finite runs; this is not an end-to-end NaN-gap guarantee for upstream extrema/view-peaks. Sample mode requires one-to-one original frames at `i / N`; repeated bins use Envelope and oversized bin input fails closed. Mode changes, resize and extreme zoom preserve sample/selection coordinates and do not pause capture.
- Verify preference/project persistence per desktop/plugin instance, separate Sender instances, new Release payload/Setup content and actual installed executable hashes. A successful code test is not a blanket no-dropout/no-artifact claim.

Historical reports and evidence directories remain immutable for this round. New test/build evidence belongs to this plan or a separate new record, not rewritten old measurements.
