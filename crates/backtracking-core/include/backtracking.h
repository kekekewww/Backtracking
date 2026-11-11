#ifndef BACKTRACKING_H
#define BACKTRACKING_H

#include <stdint.h>

typedef struct BtCore BtCore;
typedef struct BtTracks BtTracks;
typedef struct BtSnapshot BtSnapshot;

typedef struct BtMeterReadings {
    float peak_dbfs;
    float vu;
    float momentary_lufs;
} BtMeterReadings;

// Call controls and bt_poll on one thread, and push from exactly one audio
// producer. The input arrays must contain the requested number of frames.
// Stop and join the producer before rearming after a pause, changing devices,
// or freeing the handle.
// History durations are 10, 20, 30, or 60 seconds.
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
BtMeterReadings bt_tracks_meter_readings(const BtTracks *core, uint32_t lane);
uint64_t bt_tracks_waveform(const BtTracks *core, uint32_t lane, uint32_t view,
                            float *minimum, float *maximum, uint32_t bins);
uint64_t bt_tracks_first_frame(const BtTracks *core);
uint64_t bt_tracks_end_frame(const BtTracks *core);
uint32_t bt_tracks_channels(const BtTracks *core, uint32_t lane);
BtSnapshot *bt_tracks_snapshot(const BtTracks *core, uint32_t lane);

#endif
