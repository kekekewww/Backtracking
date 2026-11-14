#ifndef BACKTRACKING_H
#define BACKTRACKING_H

#include <stddef.h>
#include <stdint.h>

typedef struct BtCore BtCore;
typedef struct BtTracks BtTracks;
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
// timestamp_ns is the first frame's monotonic host time, on the same clock for both lanes.
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
void bt_tracks_set_retain_silence(const BtTracks *core, uint32_t enabled);
BtMeterReadings bt_tracks_meter_readings(const BtTracks *core, uint32_t lane);
uint64_t bt_tracks_waveform(const BtTracks *core, uint32_t lane, uint32_t view,
                            float *minimum, float *maximum, uint32_t bins);
uint64_t bt_tracks_first_frame(const BtTracks *core);
uint64_t bt_tracks_end_frame(const BtTracks *core);
uint32_t bt_tracks_channels(const BtTracks *core, uint32_t lane);
BtSnapshot *bt_tracks_snapshot(const BtTracks *core, uint32_t lane);
BtSnapshot *bt_tracks_snapshot_range(const BtTracks *core, uint32_t lane, uint64_t first, uint64_t end);

uint32_t bt_tracks_sample_rate(const BtTracks *core);
uint64_t bt_tracks_waveform_range(const BtTracks *core, uint32_t lane, uint32_t view,
                                  uint64_t first, uint64_t end, float *minimum, float *maximum, uint32_t bins);
size_t bt_tracks_copy_bar_markers(const BtTracks *core, uint64_t first, uint64_t end,
                                   uint64_t *markers, size_t capacity);
int32_t bt_tracks_attach_sender(const BtTracks *core, uint32_t lane, const char *id, uint32_t route);
void bt_tracks_detach_sender(const BtTracks *core, uint32_t lane);
BtSenders *bt_senders_new(int32_t audio_only);
void bt_senders_free(BtSenders *senders);
size_t bt_senders_count(const BtSenders *senders);
char *bt_senders_string(const BtSenders *senders, size_t index, int32_t name);
uint32_t bt_senders_format(const BtSenders *senders, size_t index, uint32_t *channels);
int32_t bt_sender_request_snapshot(const char *id);

#endif
