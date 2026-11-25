#ifndef BACKTRACKING_POSITIONS_H
#define BACKTRACKING_POSITIONS_H
#include <stdint.h>
#include <stdbool.h>
#ifdef __cplusplus
extern "C" {
#endif
typedef struct BtPositionTrack BtPositionTrack;
typedef struct BtFrozenPositions BtFrozenPositions;
typedef struct { uint64_t frame; double x,y,z,size,gain; } BtPositionKey;
/* Same monotonic nanosecond clock as Sender audio packets and positions_since. */
uint64_t bt_capture_clock_ns(void);
/* One track per lane. Serialize access to a mutable track. */
BtPositionTrack *bt_positions_new(void);
void bt_positions_free(BtPositionTrack *);
bool bt_positions_insert(BtPositionTrack *, uint32_t channel, BtPositionKey key);
/* Report a capture frame-clock eviction before retaining or freezing this lane.
   The floor only advances until clear. Frozen keys below it are omitted. */
void bt_positions_clock_evicted_before(BtPositionTrack *, uint64_t first);
void bt_positions_retain(BtPositionTrack *, uint64_t first, uint64_t end);
void bt_positions_clear(BtPositionTrack *);
/* Reports a key-budget or clock eviction once per history (reset by clear). */
bool bt_positions_take_overflowed(BtPositionTrack *);
BtFrozenPositions *bt_positions_freeze(const BtPositionTrack *);
void bt_frozen_positions_free(BtFrozenPositions *);
/* Half-open frame range, including the preceding key for interpolation.
   Returns required count. Writes at most capacity keys; NULL queries the count. */
uintptr_t bt_frozen_positions_read(const BtFrozenPositions *, uint32_t channel,
    uint64_t first, uint64_t end, BtPositionKey *out, uintptr_t capacity);
#ifdef __cplusplus
}
#endif
#endif
