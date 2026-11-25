//! The C interface to selection previews and fades, for the macOS app, which edits snapshots
//! and draws its waveform natively.

use std::ptr;
use std::slice;

use crate::{dragged_fades, fade_waveform_bins, ChannelRoute, FadeEnd, Fades, Preview, PreviewSlot, Snapshot};

#[no_mangle]
extern "C" fn bt_preview_slot_new() -> *mut PreviewSlot {
    Box::into_raw(Box::new(PreviewSlot::new()))
}

/// A slot whose renders hold a gate of this process alone, so other Backtracking processes
/// keep recording System playback while it renders (for checks that render offline).
#[no_mangle]
extern "C" fn bt_preview_slot_new_local() -> *mut PreviewSlot {
    let gate: &'static crate::preview::PreviewGate = Box::leak(Box::new(crate::preview::PreviewGate::local()));
    Box::into_raw(Box::new(PreviewSlot::with_gate(gate)))
}

/// Frees the slot and any preview in it. Nothing may render the slot any more.
#[no_mangle]
unsafe extern "C" fn bt_preview_slot_free(slot: *mut PreviewSlot) {
    if !slot.is_null() {
        // Waits out a render that a device stopped a moment ago may still be finishing.
        (*slot).clear();
        drop(Box::from_raw(slot));
    }
}

/// Plays `first_frame..end_frame` of one or two snapshots, routed as their WAVs are exported,
/// in place of whatever the slot held. Returns 1 when it plays and 0, leaving the slot as it
/// was, when the snapshots do not all hold the selection at one rate or a route does not fit.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn bt_preview_slot_play(
    slot: *const PreviewSlot,
    snapshots: *const *const Snapshot,
    count: usize,
    routes: *const u32,
    first_frame: u64,
    end_frame: u64,
    force_mono: u32,
    fades: Fades,
) -> i32 {
    let Some(slot) = slot.as_ref() else { return 0 };
    if snapshots.is_null() || routes.is_null() || !(1..=2).contains(&count) {
        return 0;
    }
    let Some(snapshots) = slice::from_raw_parts(snapshots, count)
        .iter()
        .map(|&snapshot| snapshot.as_ref())
        .collect::<Option<Vec<&Snapshot>>>()
    else {
        return 0;
    };
    let Some(routes) = slice::from_raw_parts(routes, count)
        .iter()
        .map(|&route| ChannelRoute::from_code(route))
        .collect::<Option<Vec<_>>>()
    else {
        return 0;
    };
    if first_frame >= end_frame {
        return 0;
    }
    // SAFETY: the header asks the caller to keep the snapshots alive while the slot holds them.
    let preview = Preview::from_snapshots(
        &snapshots, first_frame..end_frame, &routes, force_mono != 0, fades,
    );
    match preview {
        Some(preview) => {
            slot.play(preview);
            1
        }
        None => 0,
    }
}

/// Asks the preview to fall silent over a few milliseconds, or lets it go at once when nothing
/// has rendered it lately.
#[no_mangle]
unsafe extern "C" fn bt_preview_slot_stop(slot: *const PreviewSlot) {
    if let Some(slot) = slot.as_ref() {
        slot.stop();
    }
}

/// Holds the preview gate `nanoseconds` longer after each render, for an output that plays a
/// frame that much later than it is rendered. At most five seconds count.
#[no_mangle]
unsafe extern "C" fn bt_preview_slot_set_output_latency(slot: *const PreviewSlot, nanoseconds: u64) {
    if let Some(slot) = slot.as_ref() {
        slot.set_output_latency(std::time::Duration::from_nanos(nanoseconds));
    }
}

/// Lets go of the preview at once. Returns once no render reads it.
#[no_mangle]
unsafe extern "C" fn bt_preview_slot_clear(slot: *const PreviewSlot) {
    if let Some(slot) = slot.as_ref() {
        slot.clear();
    }
}

/// Lets go of a preview that has finished, or that was stopped and that nothing renders.
/// Returns 1 while the slot still holds one.
#[no_mangle]
unsafe extern "C" fn bt_preview_slot_reap(slot: *const PreviewSlot) -> u32 {
    slot.as_ref().map_or(0, |slot| u32::from(slot.reap()))
}

/// Changes the ramps of the playing preview from its next rendered block on.
#[no_mangle]
unsafe extern "C" fn bt_preview_slot_set_fades(slot: *const PreviewSlot, fades: Fades) {
    if let Some(slot) = slot.as_ref() {
        slot.with_current(|preview| preview.set_fades(fades));
    }
}

/// Writes the timeline frame the preview has rendered up to. Returns 0 when the slot is empty
/// or its preview has finished, 1 while it plays and 2 while it falls silent after a stop.
#[no_mangle]
unsafe extern "C" fn bt_preview_slot_position(slot: *const PreviewSlot, frame: *mut u64) -> u32 {
    let Some(slot) = slot.as_ref() else { return 0 };
    let state = slot.with_current(|preview| {
        let state = match (preview.is_finished(), preview.stop_requested()) {
            (true, _) => 0,
            (false, false) => 1,
            (false, true) => 2,
        };
        (state, preview.frames().start + preview.rendered())
    });
    let Some((state, position)) = state else { return 0 };
    if let Some(frame) = frame.as_mut() {
        *frame = position;
    }
    state
}

/// Renders `frames` frames of the preview into `left` and `right` and returns how many frames
/// it still has to play. Frames it has no audio for are silent, and so is a preview captured
/// at another rate than `sample_rate`. Pass the same buffer, or a null `right`, for one
/// channel that hears both sides. For the audio thread: it never locks, allocates or waits.
#[no_mangle]
unsafe extern "C" fn bt_preview_slot_render(
    slot: *const PreviewSlot,
    left: *mut f32,
    right: *mut f32,
    frames: u32,
    sample_rate: u32,
) -> u64 {
    if left.is_null() {
        return 0;
    }
    let frames = frames as usize;
    let right = if right == left { ptr::null_mut() } else { right };
    // Raw writes, because the two outputs may be one buffer.
    ptr::write_bytes(left, 0, frames);
    if !right.is_null() {
        ptr::write_bytes(right, 0, frames);
    }
    let Some(slot) = slot.as_ref() else { return 0 };
    slot.render_with(frames, sample_rate, |index, [l, r]| {
        if right.is_null() {
            *left.add(index) = (l + r) * 0.5;
        } else {
            *left.add(index) = l;
            *right.add(index) = r;
        }
    })
    .remaining
}

/// The fades after dragging a selection flag (`end` 0 for the start flag's fade in, 1 for the
/// end flag's fade out) from frame `press` to frame `pointer`, starting from `initial`.
#[no_mangle]
extern "C" fn bt_fades_dragged(
    end: u32,
    first_frame: u64,
    end_frame: u64,
    initial: Fades,
    press: u64,
    pointer: u64,
) -> Fades {
    let end = if end == 0 { FadeEnd::In } else { FadeEnd::Out };
    dragged_fades(end, &(first_frame..end_frame.max(first_frame)), initial, press, pointer)
}

/// The ramps shortened so that together they fit a clip of `length` frames.
#[no_mangle]
extern "C" fn bt_fades_fitted(fades: Fades, length: u64) -> Fades {
    fades.fitted(length)
}

/// Scales `bins` waveform bins over `visible_first..visible_end` by the fades of the selection
/// `selection_first..selection_end`, so the drawn waveform tapers as the exported one does.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn bt_waveform_fade_bins(
    minimum: *mut f32,
    maximum: *mut f32,
    bins: u32,
    visible_first: u64,
    visible_end: u64,
    selection_first: u64,
    selection_end: u64,
    fades: Fades,
) {
    if minimum.is_null() || maximum.is_null() || minimum == maximum {
        return;
    }
    let minimum = slice::from_raw_parts_mut(minimum, bins as usize);
    let maximum = slice::from_raw_parts_mut(maximum, bins as usize);
    fade_waveform_bins(
        minimum, maximum, visible_first..visible_end.max(visible_first),
        &(selection_first..selection_end.max(selection_first)), fades,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The route code of a lane exported as it was captured.
    const MERGED: u32 = 5;

    fn snapshot(first_frame: u64, frames: usize) -> Snapshot {
        Snapshot { sample_rate: 48_000, channels: 1, samples: vec![1.0; frames], first_frame }
    }

    #[test]
    fn a_slot_plays_a_snapshot_selection_and_reports_its_position() {
        let slot = bt_preview_slot_new();
        let source = snapshot(1_000, 1_000);
        let snapshots = [&source as *const Snapshot];
        let routes = [MERGED];
        unsafe {
            assert_eq!(bt_preview_slot_position(slot, ptr::null_mut()), 0);
            // Out of the snapshot, empty, or with an unknown route: nothing plays.
            assert_eq!(bt_preview_slot_play(slot, snapshots.as_ptr(), 1, routes.as_ptr(), 900, 1_100, 0,
                Fades::NONE), 0);
            assert_eq!(bt_preview_slot_play(slot, snapshots.as_ptr(), 1, routes.as_ptr(), 1_100, 1_100, 0,
                Fades::NONE), 0);
            assert_eq!(bt_preview_slot_play(slot, snapshots.as_ptr(), 1, [99].as_ptr(), 1_100, 1_200, 0,
                Fades::NONE), 0);
            assert_eq!(bt_preview_slot_play(slot, snapshots.as_ptr(), 1, routes.as_ptr(), 1_100, 1_200, 0,
                Fades::new(10, 0)), 1);

            let mut frame = 0;
            assert_eq!(bt_preview_slot_position(slot, &mut frame), 1);
            assert_eq!(frame, 1_100);
            let (mut left, mut right) = ([9.0f32; 64], [9.0f32; 64]);
            assert_eq!(bt_preview_slot_render(slot, left.as_mut_ptr(), right.as_mut_ptr(), 64, 48_000), 36);
            assert_eq!(left[0], 0.0);
            assert_eq!(left[5], 0.5);
            assert_eq!(left[63], 1.0);
            assert_eq!(left, right);
            assert_eq!(bt_preview_slot_position(slot, &mut frame), 1);
            assert_eq!(frame, 1_164);

            // One buffer for both sides, past the end silent.
            let mut mono = [9.0f32; 64];
            assert_eq!(bt_preview_slot_render(slot, mono.as_mut_ptr(), mono.as_mut_ptr(), 64, 48_000), 0);
            assert_eq!(mono[35], 1.0);
            assert!(mono[36..].iter().all(|&sample| sample == 0.0));
            assert_eq!(bt_preview_slot_position(slot, &mut frame), 0);
            assert_eq!(bt_preview_slot_reap(slot), 0);
            bt_preview_slot_free(slot);
        }
    }

    #[test]
    fn a_stopped_slot_falls_silent_and_another_rate_renders_silence() {
        let slot = bt_preview_slot_new();
        let source = snapshot(0, 48_000);
        let snapshots = [&source as *const Snapshot];
        let routes = [MERGED];
        unsafe {
            assert_eq!(bt_preview_slot_play(slot, snapshots.as_ptr(), 1, routes.as_ptr(), 0, 48_000, 0,
                Fades::NONE), 1);
            let mut buffer = [9.0f32; 128];
            assert_eq!(bt_preview_slot_render(slot, buffer.as_mut_ptr(), ptr::null_mut(), 128, 44_100), 0);
            assert!(buffer.iter().all(|&sample| sample == 0.0));
            // Nothing has played it, so a stop lets it go at once.
            bt_preview_slot_stop(slot);
            assert_eq!(bt_preview_slot_position(slot, ptr::null_mut()), 0);
            assert_eq!(bt_preview_slot_reap(slot), 0);

            // One that plays falls silent over a few milliseconds.
            assert_eq!(bt_preview_slot_play(slot, snapshots.as_ptr(), 1, routes.as_ptr(), 0, 48_000, 0,
                Fades::NONE), 1);
            assert!(bt_preview_slot_render(slot, buffer.as_mut_ptr(), ptr::null_mut(), 128, 48_000) > 0);
            bt_preview_slot_stop(slot);
            assert_eq!(bt_preview_slot_position(slot, ptr::null_mut()), 2);
            let mut tail = vec![0.0f32; 1_000];
            assert_eq!(bt_preview_slot_render(slot, tail.as_mut_ptr(), ptr::null_mut(), 1_000, 48_000), 0);
            // 8 ms at 48 kHz, then silence.
            assert!(tail[0] > 0.99);
            assert!(tail[384..].iter().all(|&sample| sample == 0.0));
            assert_eq!(bt_preview_slot_reap(slot), 0);
            bt_preview_slot_free(slot);
        }
    }

    #[test]
    fn fades_change_while_playing_and_drag_and_fit_as_in_core() {
        let slot = bt_preview_slot_new();
        let source = snapshot(0, 100);
        let snapshots = [&source as *const Snapshot];
        let routes = [MERGED];
        unsafe {
            assert_eq!(bt_preview_slot_play(slot, snapshots.as_ptr(), 1, routes.as_ptr(), 0, 100, 0,
                Fades::NONE), 1);
            bt_preview_slot_set_fades(slot, Fades::new(0, 50));
            let mut buffer = [0.0f32; 100];
            bt_preview_slot_render(slot, buffer.as_mut_ptr(), ptr::null_mut(), 100, 48_000);
            assert_eq!(buffer[0], 1.0);
            assert_eq!(buffer[50], 0.98);
            assert_eq!(buffer[99], 0.0);
            bt_preview_slot_clear(slot);
            assert_eq!(bt_preview_slot_reap(slot), 0);
            bt_preview_slot_free(slot);
        }
        assert_eq!(bt_fades_dragged(0, 1_000, 2_000, Fades::NONE, 1_000, 1_250), Fades::new(250, 0));
        assert_eq!(bt_fades_dragged(1, 1_000, 2_000, Fades::NONE, 2_000, 1_900), Fades::new(0, 100));
        assert_eq!(bt_fades_fitted(Fades::new(6, 6), 10), Fades::new(6, 4));

        let (mut minimum, mut maximum) = ([-1.0f32; 10], [1.0f32; 10]);
        unsafe {
            bt_waveform_fade_bins(minimum.as_mut_ptr(), maximum.as_mut_ptr(), 10, 0, 10, 2, 8,
                Fades::new(2, 2));
        }
        assert_eq!(maximum, [1.0, 1.0, 0.0, 0.5, 1.0, 1.0, 0.5, 0.0, 1.0, 1.0]);
        assert_eq!(minimum[3], -0.5);
    }
}
