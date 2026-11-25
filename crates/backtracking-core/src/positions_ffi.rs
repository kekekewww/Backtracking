use crate::{FrozenPositionTrack, Position, PositionKey, PositionTrack};
#[no_mangle]
pub extern "C" fn bt_capture_clock_ns() -> u64 {
    crate::capture_clock_ns()
}

#[repr(C)]
pub struct BtPositionKey {
    frame: u64,
    x: f64,
    y: f64,
    z: f64,
    size: f64,
    gain: f64,
}
// Mirrors PositionTimeline in desktop/windows/src/addons/position_timeline.rs
// at e9a4efc0d2918bccadeecf2c7e586b84ddd5a4f4.
const TRACK_BYTES: usize = 6 * 1024 * 1024;

pub struct BtPositionTrack {
    track: PositionTrack,
    clock_floor: u64,
    overflowed: bool,
    warned: bool,
}

#[no_mangle]
pub extern "C" fn bt_positions_new() -> *mut BtPositionTrack {
    Box::into_raw(Box::new(BtPositionTrack {
        track: PositionTrack::with_memory_limit(TRACK_BYTES),
        clock_floor: 0,
        overflowed: false,
        warned: false,
    }))
}
#[no_mangle]
pub unsafe extern "C" fn bt_positions_free(p: *mut BtPositionTrack) {
    if !p.is_null() {
        drop(Box::from_raw(p))
    }
}
#[no_mangle]
pub unsafe extern "C" fn bt_positions_insert(
    p: *mut BtPositionTrack,
    channel: u32,
    k: BtPositionKey,
) -> bool {
    let Some(t) = p.as_mut() else {
        return false;
    };
    if channel >= 64
        || ![k.x, k.y, k.z, k.size, k.gain]
            .iter()
            .all(|n| n.is_finite())
    {
        return false;
    }
    t.track.insert(
        channel as usize,
        PositionKey {
            frame: k.frame,
            position: Position {
                x: k.x,
                y: k.y,
                z: k.z,
                size: k.size,
                gain: k.gain,
            },
        },
    );
    true
}
#[no_mangle]
pub unsafe extern "C" fn bt_positions_retain(p: *mut BtPositionTrack, first: u64, end: u64) {
    if let Some(t) = p.as_mut() {
        t.track.retain_frames(first.max(t.clock_floor)..end)
    }
}
#[no_mangle]
pub unsafe extern "C" fn bt_positions_clear(p: *mut BtPositionTrack) {
    if let Some(t) = p.as_mut() {
        t.track.clear();
        t.clock_floor = 0;
        t.overflowed = false;
        t.warned = false;
    }
}
#[no_mangle]
pub unsafe extern "C" fn bt_positions_clock_evicted_before(p: *mut BtPositionTrack, first: u64) {
    if let Some(t) = p.as_mut() {
        t.clock_floor = t.clock_floor.max(first);
        t.overflowed = true;
    }
}
#[no_mangle]
pub unsafe extern "C" fn bt_positions_take_overflowed(p: *mut BtPositionTrack) -> bool {
    let Some(t) = p.as_mut() else { return false };
    t.overflowed |= t.track.take_overflowed();
    t.overflowed && !std::mem::replace(&mut t.warned, true)
}
#[no_mangle]
pub unsafe extern "C" fn bt_positions_freeze(
    p: *const BtPositionTrack,
) -> *mut FrozenPositionTrack {
    p.as_ref().map_or(std::ptr::null_mut(), |t| {
        let floor = t.clock_floor;
        let has_stale_key =
            floor > 0 && (0..64).any(|ch| !t.track.channel_positions(ch, 0..floor).is_empty());
        let frozen = if has_stale_key {
            let mut visible = PositionTrack::with_memory_limit(TRACK_BYTES);
            for ch in 0..64 {
                for key in t.track.channel_positions(ch, floor..u64::MAX) {
                    if key.frame >= floor {
                        visible.insert(ch, key);
                    }
                }
            }
            visible.freeze()
        } else {
            t.track.freeze()
        };
        Box::into_raw(Box::new(frozen))
    })
}
#[no_mangle]
pub unsafe extern "C" fn bt_frozen_positions_free(p: *mut FrozenPositionTrack) {
    if !p.is_null() {
        drop(Box::from_raw(p))
    }
}
#[no_mangle]
pub unsafe extern "C" fn bt_frozen_positions_read(
    p: *const FrozenPositionTrack,
    channel: u32,
    first: u64,
    end: u64,
    out: *mut BtPositionKey,
    capacity: usize,
) -> usize {
    let Some(track) = p.as_ref() else { return 0 };
    let keys = track.channel_positions(channel as usize, first..end);
    if !out.is_null() {
        for (i, k) in keys.iter().take(capacity).enumerate() {
            let Position {
                x,
                y,
                z,
                size,
                gain,
            } = k.position;
            out.add(i).write(BtPositionKey {
                frame: k.frame,
                x,
                y,
                z,
                size,
                gain,
            });
        }
    }
    keys.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(frame: u64) -> BtPositionKey {
        BtPositionKey {
            frame,
            x: 0.,
            y: 0.,
            z: 0.,
            size: 0.,
            gain: 1.,
        }
    }

    #[test]
    fn eviction_filters_predecessors_across_channels_and_warns_once_until_clear() {
        unsafe {
            let live = bt_positions_new();
            assert!(!bt_positions_take_overflowed(live));
            for frame in [2, 4, 6] {
                assert!(bt_positions_insert(live, 1, key(frame)));
            }
            assert!(bt_positions_insert(live, 0, key(3)));
            bt_positions_retain(live, 4, 10);
            let before = bt_positions_freeze(live);
            bt_positions_clock_evicted_before(live, 4);
            bt_positions_clock_evicted_before(live, 2);
            bt_positions_retain(live, 1, 10);
            let after = bt_positions_freeze(live);
            assert_eq!((*before).len(), 4);
            assert_eq!((*after).len(), 2);
            assert_eq!((*after).channel_positions(1, 0..10)[0].frame, 4);
            assert!(bt_positions_take_overflowed(live));
            bt_positions_clock_evicted_before(live, 8);
            assert!(!bt_positions_take_overflowed(live));
            bt_positions_retain(live, 0, 7);
            assert!((*live).track.is_empty());
            bt_positions_clear(live);
            assert!(!bt_positions_take_overflowed(live));
            assert!(bt_positions_insert(live, 0, key(1)));
            let reset = bt_positions_freeze(live);
            assert_eq!((*reset).len(), 1);
            bt_positions_clock_evicted_before(live, 1);
            assert!(bt_positions_take_overflowed(live));
            bt_positions_free(live);
            for frozen in [before, after, reset] {
                bt_frozen_positions_free(frozen);
            }
        }
    }

    #[test]
    fn six_mib_budget_and_clock_share_one_warning() {
        unsafe {
            let live = bt_positions_new();
            // 140000 keys alone exceed 6 MiB but fit the core's normal 8 MiB.
            for n in 0..140_000 {
                assert!(bt_positions_insert(live, (n % 64) as u32, key(n)));
            }
            assert!((*live).track.memory_bytes() <= 6 * 1024 * 1024);
            assert!(bt_positions_take_overflowed(live));
            bt_positions_clock_evicted_before(live, 5);
            assert!(!bt_positions_take_overflowed(live));
            bt_positions_free(live);
        }
    }

    #[test]
    fn freezing_is_independent_and_truncated_reads_return_the_full_count() {
        unsafe {
            let live = bt_positions_new();
            for frame in [10, 20, 30] {
                assert!(bt_positions_insert(
                    live,
                    0,
                    BtPositionKey {
                        frame,
                        x: 1.,
                        y: 0.,
                        z: 0.,
                        size: 0.,
                        gain: 1.
                    }
                ));
            }
            let frozen = bt_positions_freeze(live);
            bt_positions_clear(live);
            bt_positions_free(live);
            let mut out = std::mem::MaybeUninit::uninit();
            assert_eq!(
                bt_frozen_positions_read(frozen, 0, 15, 30, out.as_mut_ptr(), 1),
                2
            );
            assert_eq!(out.assume_init().frame, 10);
            bt_frozen_positions_free(frozen);
        }
    }
}
