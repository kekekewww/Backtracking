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
#[no_mangle]
pub extern "C" fn bt_positions_new() -> *mut PositionTrack {
    Box::into_raw(Box::new(PositionTrack::new()))
}
#[no_mangle]
pub unsafe extern "C" fn bt_positions_free(p: *mut PositionTrack) {
    if !p.is_null() {
        drop(Box::from_raw(p))
    }
}
#[no_mangle]
pub unsafe extern "C" fn bt_positions_insert(
    p: *mut PositionTrack,
    channel: u32,
    k: BtPositionKey,
) -> bool {
    let Some(track) = p.as_mut() else {
        return false;
    };
    if channel >= 64
        || ![k.x, k.y, k.z, k.size, k.gain]
            .iter()
            .all(|n| n.is_finite())
    {
        return false;
    }
    track.insert(
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
pub unsafe extern "C" fn bt_positions_retain(p: *mut PositionTrack, first: u64, end: u64) {
    if let Some(t) = p.as_mut() {
        t.retain_frames(first..end)
    }
}
#[no_mangle]
pub unsafe extern "C" fn bt_positions_clear(p: *mut PositionTrack) {
    if let Some(t) = p.as_mut() {
        t.clear()
    }
}
#[no_mangle]
pub unsafe extern "C" fn bt_positions_take_overflowed(p: *mut PositionTrack) -> bool {
    p.as_mut().is_some_and(PositionTrack::take_overflowed)
}
#[no_mangle]
pub unsafe extern "C" fn bt_positions_freeze(p: *const PositionTrack) -> *mut FrozenPositionTrack {
    p.as_ref().map_or(std::ptr::null_mut(), |t| {
        Box::into_raw(Box::new(t.freeze()))
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
