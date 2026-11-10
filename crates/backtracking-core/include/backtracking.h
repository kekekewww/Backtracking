#ifndef BACKTRACKING_H
#define BACKTRACKING_H

#include <stdint.h>

typedef struct BtCore BtCore;
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

BtSnapshot *bt_snapshot(const BtCore *core);
void bt_snapshot_free(BtSnapshot *snapshot);
// A frozen snapshot can be passed to a worker for export. The two endpoints
// are fractions of its frame count. The returned UTF-8 path belongs to Rust.
char *bt_snapshot_export(const BtSnapshot *snapshot, double start, double end, const char *directory);
// Rates are 44100, 48000 or 96000. Sample sizes 16 and 24 are PCM, 32 is IEEE float.
char *bt_snapshot_export_with_profile(const BtSnapshot *snapshot, double start, double end,
                                      const char *directory, uint32_t sample_rate,
                                      uint32_t bits_per_sample);
void bt_string_free(char *string);

#endif
