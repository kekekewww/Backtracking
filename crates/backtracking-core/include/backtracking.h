#ifndef BACKTRACKING_H
#define BACKTRACKING_H

#include <stddef.h>
#include <stdint.h>

typedef struct BtCore BtCore;
typedef struct BtTracks BtTracks;
typedef struct BtFrozenCapture BtFrozenCapture;
typedef struct BtPreviewSlot BtPreviewSlot;
typedef struct BtSnapshot BtSnapshot;
typedef struct BtSenders BtSenders;

typedef struct BtMeterReadings {
    float peak_dbfs;
    float vu;
    float momentary_lufs;
} BtMeterReadings;

typedef struct BtWaveformDisplayPolicy {
    uint32_t bins;
    float waveform_stroke;
    float bar_stroke;
    float seconds_stroke;
    float bar_spacing_px;
    float seconds_spacing_px;
    float plot_width;
} BtWaveformDisplayPolicy;

typedef struct BtTimelineGuideCounts {
    size_t bars;
    size_t seconds;
} BtTimelineGuideCounts;

// Frames over which a selection fades in from its start and out to its end, at its own rate.
// Ramps are linear; the outermost frame of each is silent.
typedef struct BtFades {
    uint64_t fade_in;
    uint64_t fade_out;
} BtFades;

// Share display density and the exact drawn/snap targets across native UIs.
BtWaveformDisplayPolicy bt_waveform_display_policy(float width, float height,
                                                   uint64_t visible_frames, uint32_t sample_rate,
                                                   uint32_t interactive);
// Return required output lengths; copy min(required, capacity) into non-null outputs.
BtTimelineGuideCounts bt_waveform_timeline_guides(uint64_t first, uint64_t end,
                                                 uint32_t sample_rate, uint32_t seconds_interval,
                                                 const uint64_t *bars, size_t bar_count,
                                                 uint32_t show_bars, uint32_t show_seconds,
                                                 uint32_t background_visible,
                                                 const BtWaveformDisplayPolicy *policy,
                                                 uint64_t *bar_output, size_t bar_capacity,
                                                 uint64_t *seconds_output, size_t seconds_capacity);
double bt_waveform_snap_visible(double fraction, uint64_t first, uint64_t end,
                                const uint64_t *flags, size_t flag_count,
                                const uint64_t *bars, size_t bar_count,
                                const uint64_t *seconds, size_t seconds_count,
                                double tolerance_fraction);

// Call controls and bt_poll on one thread, and push from exactly one audio
// producer. The input arrays must contain the requested number of frames.
// Stop and join the producer before rearming after a pause, changing devices,
// or freeing the handle.
// History durations are 10, 20, 30, 60, 180, 300, or 600 seconds.
BtCore *bt_new(double sample_rate, uint32_t channels, uint32_t history_secs);
void bt_free(BtCore *core);
uint32_t bt_push_interleaved(const BtCore *core, const float *data, uint32_t frames);
uint32_t bt_push_planar_stereo(const BtCore *core, const float *left, const float *right, uint32_t frames);
void bt_poll(const BtCore *core);
void bt_pause(const BtCore *core);
void bt_resume(const BtCore *core);
// Discard history, pre-roll and already published audio. Preserve armed/paused mode.
void bt_clear(const BtCore *core);
// Unavailable readings (including pause) are negative infinity.
BtMeterReadings bt_meter_readings(const BtCore *core);
int32_t bt_set_history(const BtCore *core, uint32_t seconds);
// Nonzero keeps future quiet input. Default off; preserve retained audio and pause.
void bt_set_retain_silence(const BtCore *core, uint32_t enabled);
uint32_t bt_state(const BtCore *core);
uint64_t bt_revision(const BtCore *core);
// The two output arrays must be disjoint and each contain at least bins floats.
uint64_t bt_waveform(const BtCore *core, float *minimum, float *maximum, uint32_t bins);
// Waveform views: 0 = mono fold-down, 1 = left, 2 = right.
uint64_t bt_waveform_view(const BtCore *core, uint32_t view, float *minimum, float *maximum, uint32_t bins);
uint64_t bt_first_frame(const BtCore *core);
uint64_t bt_end_frame(const BtCore *core);

BtSnapshot *bt_snapshot(const BtCore *core);
void bt_snapshot_free(BtSnapshot *snapshot);
uint64_t bt_snapshot_first_frame(const BtSnapshot *snapshot);
uint64_t bt_snapshot_frame_count(const BtSnapshot *snapshot);
uint32_t bt_snapshot_channels(const BtSnapshot *snapshot);
uint64_t bt_snapshot_waveform(const BtSnapshot *snapshot, uint32_t view, float *minimum,
                              float *maximum, uint32_t bins);
uint64_t bt_snapshot_waveform_range(const BtSnapshot *snapshot, uint32_t view,
                                    uint64_t first, uint64_t end,
                                    float *minimum, float *maximum, uint32_t bins);
// A frozen snapshot can be passed to a worker for export. The two endpoints
// are fractions of its frame count. The returned UTF-8 path belongs to Rust.
char *bt_snapshot_export(const BtSnapshot *snapshot, double start, double end, const char *directory);
// Rates are 44100, 48000 or 96000. Sample sizes 16 and 24 are PCM, 32 is IEEE float.
char *bt_snapshot_export_with_profile(const BtSnapshot *snapshot, double start, double end,
                                      const char *directory, uint32_t sample_rate,
                                      uint32_t bits_per_sample);
// Direct frame exports accept absolute, half-open retained-audio boundaries.
char *bt_snapshot_export_frames_with_profile(const BtSnapshot *snapshot, uint64_t first_frame,
                                             uint64_t end_frame, const char *directory,
                                             uint32_t sample_rate, uint32_t bits_per_sample);
int32_t bt_snapshots_export_pair_frames_with_profile(const BtSnapshot *snapshot_a,
                                                     const BtSnapshot *snapshot_b,
                                                     uint64_t first_frame, uint64_t end_frame,
                                                     const char *directory, uint32_t sample_rate,
                                                     uint32_t bits_per_sample, char **path_a, char **path_b);
// Routes describe already-extracted samples: 0 = Left, 1 = Right, 2 = Center,
// 3 = Mid, 4 = Side, 5 = Merged. Nonzero force_mono applies to each WAV separately.
char *bt_snapshot_export_routed_frames_with_profile(const BtSnapshot *snapshot, uint64_t first_frame,
                                                    uint64_t end_frame, const char *directory,
                                                    uint32_t sample_rate, uint32_t bits_per_sample,
                                                    uint32_t route, uint32_t force_mono);
int32_t bt_snapshots_export_pair_routed_frames_with_profile(const BtSnapshot *snapshot_a,
                                                            const BtSnapshot *snapshot_b,
                                                            uint64_t first_frame, uint64_t end_frame,
                                                            const char *directory, uint32_t sample_rate,
                                                            uint32_t bits_per_sample, uint32_t route_a,
                                                            uint32_t route_b, uint32_t force_mono,
                                                            char **path_a, char **path_b);
// The same exports with a fade in and a fade out, in the snapshot's own frames. Ramps that
// do not fit the clip are shortened to fit it.
char *bt_snapshot_export_routed_faded_frames(const BtSnapshot *snapshot, uint64_t first_frame,
                                             uint64_t end_frame, const char *directory,
                                             uint32_t sample_rate, uint32_t bits_per_sample,
                                             uint32_t route, uint32_t force_mono,
                                             uint64_t fade_in, uint64_t fade_out);
int32_t bt_snapshots_export_pair_routed_faded_frames(const BtSnapshot *snapshot_a,
                                                     const BtSnapshot *snapshot_b,
                                                     uint64_t first_frame, uint64_t end_frame,
                                                     const char *directory, uint32_t sample_rate,
                                                     uint32_t bits_per_sample, uint32_t route_a,
                                                     uint32_t route_b, uint32_t force_mono,
                                                     uint64_t fade_in, uint64_t fade_out,
                                                     char **path_a, char **path_b);
// The faded exports, with each file named after its source and lane:
// Backtracking_<source>_<lane>_<date>_<time>_<length>s.wav. A source name is UTF-8 and may be
// NULL or empty to leave it out; the lane then follows the take, as in the unnamed exports:
// Backtracking_<date>_<time>_<length>s-A.wav. lane is 0 for a single source, 1 for A through
// 64 for BL, and is given whenever multiple sources record.
char *bt_snapshot_export_labelled_frames(const BtSnapshot *snapshot, uint64_t first_frame,
                                         uint64_t end_frame, const char *directory,
                                         uint32_t sample_rate, uint32_t bits_per_sample,
                                         uint32_t route, uint32_t force_mono,
                                         uint64_t fade_in, uint64_t fade_out,
                                         const char *source_name, uint32_t lane);
int32_t bt_snapshots_export_pair_labelled_frames(const BtSnapshot *snapshot_a,
                                                 const BtSnapshot *snapshot_b,
                                                 uint64_t first_frame, uint64_t end_frame,
                                                 const char *directory, uint32_t sample_rate,
                                                 uint32_t bits_per_sample, uint32_t route_a,
                                                 uint32_t route_b, uint32_t force_mono,
                                                 uint64_t fade_in, uint64_t fade_out,
                                                 const char *source_name_a,
                                                 const char *source_name_b,
                                                 char **path_a, char **path_b);
// How those files' names begin, up to the date, such as "Backtracking_System audio_A_", for
// showing in settings; "Backtracking_" without a source name. Free the result with
// bt_string_free. NULL for an unknown lane.
char *bt_take_file_prefix(const char *source_name, uint32_t lane);
int32_t bt_snapshots_export_pair_with_profile(const BtSnapshot *snapshot_a,
                                              const BtSnapshot *snapshot_b,
                                              double start, double end,
                                              const char *directory, uint32_t sample_rate,
                                              uint32_t bits_per_sample, char **path_a, char **path_b);
void bt_string_free(char *string);

// Call controls and bt_tracks_poll on one thread. Each lane (0 = A, 1 = B)
// can be fed from its own single producer. Stop producers before freeing or
// rearming the handle. Set b_channels to zero for single-lane capture.
BtTracks *bt_tracks_new(double sample_rate, uint32_t a_channels, uint32_t b_channels, uint32_t history_secs);
void bt_tracks_free(BtTracks *core);
// timestamp_ns is the first frame's monotonic host time, on the same clock for all lanes.
uint32_t bt_tracks_push_interleaved(const BtTracks *core, uint32_t lane, const float *data,
                                    uint32_t frames, uint64_t timestamp_ns, double source_rate);
uint32_t bt_tracks_push_planar_stereo(const BtTracks *core, uint32_t lane, const float *left,
                                      const float *right, uint32_t frames, uint64_t timestamp_ns,
                                      double source_rate);
void bt_tracks_poll(const BtTracks *core);
void bt_tracks_pause(const BtTracks *core);
void bt_tracks_resume(const BtTracks *core);
void bt_tracks_clear(const BtTracks *core);
uint32_t bt_tracks_state(const BtTracks *core);
uint64_t bt_tracks_revision(const BtTracks *core);
int32_t bt_tracks_set_history(const BtTracks *core, uint32_t seconds);
uint64_t bt_tracks_retention_frames(const BtTracks *core);
void bt_tracks_set_retain_silence(const BtTracks *core, uint32_t enabled);
BtMeterReadings bt_tracks_meter_readings(const BtTracks *core, uint32_t lane);
uint64_t bt_tracks_waveform(const BtTracks *core, uint32_t lane, uint32_t view,
                            float *minimum, float *maximum, uint32_t bins);
uint64_t bt_tracks_first_frame(const BtTracks *core);
uint64_t bt_tracks_end_frame(const BtTracks *core);
uint32_t bt_tracks_channels(const BtTracks *core, uint32_t lane);
// Create 1 to 64 lanes with at most 64 channels total.
BtTracks *bt_tracks_new_lanes(double rate, const uint32_t *widths, size_t count,
                             uint32_t history_secs, uint64_t physical_memory);
char *bt_lane_letter(uint32_t index);

// Share immutable pages with a renderer or exporter. Release with bt_frozen_free.
// View 0/1/2 is merged/left/right, 3 + channel index is native, and 67/68 is the wide L/R pair.
// labels is a tab-separated list of native channel names, or NULL for numbered names.
BtFrozenCapture *bt_tracks_freeze(const BtTracks *core, uint64_t first, uint64_t end);
void bt_frozen_free(BtFrozenCapture *frozen);
void bt_frozen_waveform(const BtFrozenCapture *frozen, uint32_t lane, uint32_t view,
                        uint64_t first, uint64_t end, float *minimum, float *maximum, uint32_t bins,
                        const char *labels);
// The output holds six consecutive energies per bin, low to high. Silence is all zeros.
void bt_frozen_spectrum(const BtFrozenCapture *frozen, uint32_t lane, uint32_t view,
                        uint64_t first, uint64_t end, float *energies, uint32_t bins, const char *labels);

typedef struct BtFrozenLane {
    uint32_t index;
    uint32_t route;
    const char *source_name;
    const char *channel_labels; // Tab-separated, in channel order. NULL means numbered labels.
    uint32_t view; // 0 for the whole lane, otherwise the chosen row.
} BtFrozenLane;

int32_t bt_frozen_play(const BtFrozenCapture *frozen, const BtPreviewSlot *slot,
                       const BtFrozenLane *lanes, size_t count, uint64_t first, uint64_t end,
                       uint32_t force_mono, uint32_t pair_binaural, BtFades fades);
// Export formats for wide lanes: 0 binaural per channel, 1 mono per channel,
// 2 one multichannel WAV, 3 one binaural mix. A whole lane also writes its L/R mix.
typedef struct BtExportPaths BtExportPaths;
BtExportPaths *bt_frozen_export(const BtFrozenCapture *frozen, const BtFrozenLane *lanes, size_t count,
                                uint64_t first, uint64_t end, const char *directory, uint32_t sample_rate,
                                uint32_t bits_per_sample, uint32_t force_mono, uint32_t wide_mode,
                                uint32_t pair_binaural, BtFades fades);
size_t bt_export_paths_count(const BtExportPaths *paths);
// Each path is borrowed until bt_export_paths_free.
const char *bt_export_path(const BtExportPaths *paths, size_t index);
void bt_export_paths_free(BtExportPaths *paths);

// Bitmask of channels (bit i = native channel i) that have non-silent audio (> -90 dBFS)
// in the retained history or frozen capture. Returns 0 for narrow (<= 2ch) or unknown lanes.
uint64_t bt_tracks_active_channels(const BtTracks *core, uint32_t lane);
uint64_t bt_frozen_active_channels(const BtFrozenCapture *frozen, uint32_t lane);

// Multi-row / channel-subset selection for preview and export.
// When channel_mask_valid == 0, every channel of the lane is selected.
// When channel_mask_valid != 0, bit i of channel_mask selects native channel i;
// channel_mask == 0 with include_pair != 0 selects only the wide L/R pair.
typedef struct BtFrozenSelectionLane {
    uint32_t index;
    uint32_t route;
    const char *source_name;
    const char *channel_labels; // Tab-separated, in channel order. NULL means numbered labels.
    uint64_t channel_mask;
    uint32_t channel_mask_valid;
    uint32_t include_pair; // For wide lanes: nonzero includes the L/R pair WAV.
} BtFrozenSelectionLane;

// Like bt_frozen_play, supporting arbitrary channel subsets per lane and returning a UTF-8
// error message via error_out on failure (free with bt_string_free).
int32_t bt_frozen_play_selection(const BtFrozenCapture *frozen, const BtPreviewSlot *slot,
                                 const BtFrozenSelectionLane *lanes, size_t count,
                                 uint64_t first, uint64_t end,
                                 uint32_t force_mono, uint32_t pair_binaural, BtFades fades,
                                 char **error_out);

// Like bt_frozen_export, supporting arbitrary channel subsets per lane (cross-row selection
// and hide-silent-channels export) and returning a UTF-8 error message via error_out on failure
// (free with bt_string_free).
BtExportPaths *bt_frozen_export_selection(const BtFrozenCapture *frozen,
                                          const BtFrozenSelectionLane *lanes, size_t count,
                                          uint64_t first, uint64_t end, const char *directory,
                                          uint32_t sample_rate, uint32_t bits_per_sample,
                                          uint32_t force_mono, uint32_t wide_mode,
                                          uint32_t pair_binaural, BtFades fades,
                                          char **error_out);

BtSnapshot *bt_tracks_snapshot(const BtTracks *core, uint32_t lane);
BtSnapshot *bt_tracks_snapshot_range(const BtTracks *core, uint32_t lane, uint64_t first, uint64_t end);

uint32_t bt_tracks_sample_rate(const BtTracks *core);
// Also accepts view 3 + native channel index.
uint64_t bt_tracks_waveform_range(const BtTracks *core, uint32_t lane, uint32_t view,
                                  uint64_t first, uint64_t end, float *minimum, float *maximum, uint32_t bins);
size_t bt_tracks_copy_bar_markers(const BtTracks *core, uint64_t first, uint64_t end,
                                   uint64_t *markers, size_t capacity);
int32_t bt_tracks_attach_sender(const BtTracks *core, uint32_t lane, const char *id, uint32_t route);
void bt_tracks_detach_sender(const BtTracks *core, uint32_t lane);
// Nonzero marks a lane that records System playback: what it captures while a Backtracking
// preview may be heard, from any Backtracking process, is kept as silence.
void bt_tracks_set_preview_silence(const BtTracks *core, uint32_t lane, uint32_t enabled);

// Plays a selection back. Render the slot from one audio thread; call the rest from one other
// thread. bt_preview_slot_play reads the snapshots in place: keep them alive until the slot
// has let the preview go (after the next play, bt_preview_slot_clear, bt_preview_slot_reap
// returning 0, or bt_preview_slot_free). Stop rendering before freeing the slot.
BtPreviewSlot *bt_preview_slot_new(void);
void bt_preview_slot_free(BtPreviewSlot *slot);
// One or two snapshots with their WAV routes, as exported. Returns 1 when it plays and 0,
// leaving the slot as it was, when the snapshots do not all hold the selection at one rate.
int32_t bt_preview_slot_play(const BtPreviewSlot *slot, const BtSnapshot *const *snapshots,
                             size_t count, const uint32_t *routes, uint64_t first_frame,
                             uint64_t end_frame, uint32_t force_mono, BtFades fades);
// Falls silent over a few milliseconds, or lets go at once when nothing has rendered lately.
void bt_preview_slot_stop(const BtPreviewSlot *slot);
// Lets go at once, returning once no render reads the preview.
void bt_preview_slot_clear(const BtPreviewSlot *slot);
// Lets go of a finished preview, or of a stopped one that nothing renders. Returns 1 while the
// slot still holds one.
uint32_t bt_preview_slot_reap(const BtPreviewSlot *slot);
// Holds the preview gate this much longer after each render, for an output that plays a frame
// later than it is rendered. At most five seconds count.
void bt_preview_slot_set_output_latency(const BtPreviewSlot *slot, uint64_t nanoseconds);
void bt_preview_slot_set_fades(const BtPreviewSlot *slot, BtFades fades);
// Writes the frame rendered up to. Returns 0 when nothing plays, 1 while playing and 2 while
// falling silent after a stop.
uint32_t bt_preview_slot_position(const BtPreviewSlot *slot, uint64_t *frame);
// Real-time safe. Fills both outputs (pass one buffer, or a null right, for a single channel)
// and returns the frames still to play. A preview captured at another rate renders silence.
uint64_t bt_preview_slot_render(const BtPreviewSlot *slot, float *left, float *right,
                                uint32_t frames, uint32_t sample_rate);
// The fades after dragging a flag (end 0 = start flag, fade in; 1 = end flag, fade out) of
// the selection first_frame..end_frame from frame press to frame pointer.
BtFades bt_fades_dragged(uint32_t end, uint64_t first_frame, uint64_t end_frame,
                         BtFades initial, uint64_t press, uint64_t pointer);
BtFades bt_fades_fitted(BtFades fades, uint64_t length);
// Tapers waveform bins over the visible frames by the selection's fades.
void bt_waveform_fade_bins(float *minimum, float *maximum, uint32_t bins,
                           uint64_t visible_first, uint64_t visible_end,
                           uint64_t selection_first, uint64_t selection_end, BtFades fades);
BtSenders *bt_senders_new(int32_t audio_only);
void bt_senders_free(BtSenders *senders);
size_t bt_senders_count(const BtSenders *senders);
char *bt_senders_string(const BtSenders *senders, size_t index, int32_t name);
uint32_t bt_senders_format(const BtSenders *senders, size_t index, uint32_t *channels);
// Tab-separated channel names for a Sender, or NULL. Release with bt_string_free.
char *bt_sender_channel_labels(const char *id);
int32_t bt_sender_request_snapshot(const char *id);

// Optional housekeeping for the drag-and-drop folder. days is 0 for Off, otherwise 10, 20, 30,
// 60 or 90. bt_cleanup_poll starts a pass when one is due (a null folder means "do not clean
// now") and returns the number of files a finished pass deleted, -1 when it failed and -2 when
// no pass has finished. Only WAV files Backtracking named are ever deleted.
typedef struct BtCleanup BtCleanup;
BtCleanup *bt_cleanup_new(uint32_t days);
void bt_cleanup_free(BtCleanup *cleanup);
uint32_t bt_cleanup_days(const BtCleanup *cleanup);
void bt_cleanup_set_days(BtCleanup *cleanup, uint32_t days);
void bt_cleanup_restart(BtCleanup *cleanup);
int32_t bt_cleanup_due(const BtCleanup *cleanup);
int64_t bt_cleanup_poll(BtCleanup *cleanup, const char *folder, const char *const *keep, size_t keep_count);
int32_t bt_same_folder(const char *a, const char *b);

#endif
