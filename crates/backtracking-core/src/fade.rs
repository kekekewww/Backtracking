//! Gain ramps at the two ends of a selection: a fade in from its start flag and a fade out
//! to its end flag. The export, the preview and the waveform all read the same ramps.

use std::ops::Range;

/// How many frames each end of a clip ramps over, counted at the clip's own sample rate.
///
/// Both ramps are linear in amplitude. The outermost frame of a ramp is silent and the
/// first frame past it plays at full level, so a faded clip starts and ends at zero.
/// The layout is `BtFades` in the C header.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Fades {
    /// Frames from the clip's first frame until it plays at full level.
    pub fade_in: u64,
    /// Frames before the clip's end over which it falls back to silence.
    pub fade_out: u64,
}

impl Fades {
    pub const NONE: Self = Self { fade_in: 0, fade_out: 0 };

    pub const fn new(fade_in: u64, fade_out: u64) -> Self {
        Self { fade_in, fade_out }
    }

    pub const fn is_none(self) -> bool {
        self.fade_in == 0 && self.fade_out == 0
    }

    /// The ramps shortened so that together they fit a clip of `length` frames. The fade in
    /// keeps its length and the fade out gives way, as a ramp is never longer than the clip.
    pub fn fitted(self, length: u64) -> Self {
        let fade_in = self.fade_in.min(length);
        Self { fade_in, fade_out: self.fade_out.min(length - fade_in) }
    }

    /// The gain of clip frame `index` in a clip of `length` frames, from 0 to 1.
    pub fn gain(self, index: u64, length: u64) -> f32 {
        if index >= length {
            return 0.0;
        }
        let fades = self.fitted(length);
        let mut gain = 1.0f64;
        if index < fades.fade_in {
            gain *= index as f64 / fades.fade_in as f64;
        }
        let from_end = length - 1 - index;
        if from_end < fades.fade_out {
            gain *= from_end as f64 / fades.fade_out as f64;
        }
        gain as f32
    }

    /// Multiplies `samples`, whole interleaved frames of `channels` samples that start at
    /// clip frame `first`, by the ramps of a clip of `length` frames.
    pub fn apply(self, samples: &mut [f32], channels: usize, first: u64, length: u64) {
        if self.is_none() || channels == 0 {
            return;
        }
        let fades = self.fitted(length);
        let frames = (samples.len() / channels) as u64;
        let end = first.saturating_add(frames);
        // Only the frames inside a ramp change, so a long clip costs nothing in its middle.
        // Fitted ramps never overlap, so no frame is scaled twice.
        let ramp_in = first..end.min(fades.fade_in);
        let ramp_out = first.max(length - fades.fade_out)..end;
        for index in ramp_in.chain(ramp_out) {
            let gain = fades.gain(index, length);
            let at = ((index - first) as usize) * channels;
            for sample in &mut samples[at..at + channels] {
                *sample *= gain;
            }
        }
    }
}

/// Which ramp of a selection a flag controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FadeEnd {
    /// The start flag fades the selection in.
    In,
    /// The end flag fades the selection out.
    Out,
}

/// The fades after dragging `end`'s flag from frame `press` to frame `pointer`, starting from
/// `initial`. Moving into the selection lengthens the ramp; the two ramps never overlap.
pub fn dragged_fades(
    end: FadeEnd,
    selection: &Range<u64>,
    initial: Fades,
    press: u64,
    pointer: u64,
) -> Fades {
    let length = selection.end.saturating_sub(selection.start);
    let initial = initial.fitted(length);
    let moved = i128::from(pointer) - i128::from(press);
    let stretch = |ramp: u64, by: i128, room: u64| {
        (i128::from(ramp) + by).clamp(0, i128::from(room)) as u64
    };
    match end {
        FadeEnd::In => Fades {
            fade_in: stretch(initial.fade_in, moved, length - initial.fade_out),
            ..initial
        },
        FadeEnd::Out => Fades {
            fade_out: stretch(initial.fade_out, -moved, length - initial.fade_in),
            ..initial
        },
    }
}

/// Scales waveform bins over `visible` by the selection's fades, so the drawn waveform tapers
/// the way the exported one does. Bins outside the selection keep their height.
pub fn fade_waveform_bins(
    minimum: &mut [f32],
    maximum: &mut [f32],
    visible: Range<u64>,
    selection: &Range<u64>,
    fades: Fades,
) {
    let length = selection.end.saturating_sub(selection.start);
    let bins = minimum.len().min(maximum.len());
    let frames = visible.end.saturating_sub(visible.start);
    if fades.fitted(length).is_none() || bins == 0 || frames == 0 {
        return;
    }
    for bin in 0..bins {
        // The frame in the middle of the bin; a bin per frame lands on its own frame.
        let frame = visible.start + ((bin as f64 + 0.5) * frames as f64 / bins as f64) as u64;
        if !selection.contains(&frame) {
            continue;
        }
        let gain = fades.gain(frame - selection.start, length);
        minimum[bin] *= gain;
        maximum[bin] *= gain;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dragging_a_flag_inward_lengthens_its_ramp_up_to_the_other_ramp() {
        let selection = 1_000..2_000;
        let none = Fades::NONE;
        assert_eq!(dragged_fades(FadeEnd::In, &selection, none, 1_000, 1_250), Fades::new(250, 0));
        assert_eq!(dragged_fades(FadeEnd::Out, &selection, none, 2_000, 1_900), Fades::new(0, 100));
        // Dragging outward cannot make a ramp shorter than nothing.
        assert_eq!(dragged_fades(FadeEnd::In, &selection, none, 1_000, 900), Fades::NONE);
        assert_eq!(dragged_fades(FadeEnd::Out, &selection, none, 2_000, 2_400), Fades::NONE);
        // A ramp stops where the other begins.
        let out = Fades::new(0, 300);
        assert_eq!(dragged_fades(FadeEnd::In, &selection, out, 1_000, 1_900), Fades::new(700, 300));
        let both = Fades::new(400, 0);
        assert_eq!(dragged_fades(FadeEnd::Out, &selection, both, 2_000, 1_000), Fades::new(400, 600));
    }

    #[test]
    fn a_new_drag_adjusts_the_existing_ramp_instead_of_starting_over() {
        let selection = 0..1_000;
        let existing = Fades::new(300, 200);
        // Picking the flag up where it stands changes nothing.
        assert_eq!(dragged_fades(FadeEnd::In, &selection, existing, 2, 2), existing);
        assert_eq!(dragged_fades(FadeEnd::In, &selection, existing, 2, 52), Fades::new(350, 200));
        assert_eq!(dragged_fades(FadeEnd::Out, &selection, existing, 998, 1_048), Fades::new(300, 150));
    }

    #[test]
    fn waveform_bins_taper_with_the_fades_and_stay_whole_outside_the_selection() {
        let mut minimum = vec![-1.0f32; 10];
        let mut maximum = vec![1.0f32; 10];
        // One bin per frame; the selection is frames 2..8 with a ramp of two at each end.
        fade_waveform_bins(&mut minimum, &mut maximum, 0..10, &(2..8), Fades::new(2, 2));
        assert_eq!(maximum, [1.0, 1.0, 0.0, 0.5, 1.0, 1.0, 0.5, 0.0, 1.0, 1.0]);
        assert_eq!(minimum, maximum.iter().map(|value| -value).collect::<Vec<_>>());

        let mut untouched = vec![1.0f32; 4];
        fade_waveform_bins(&mut untouched.clone(), &mut untouched, 0..8, &(0..8), Fades::NONE);
        assert_eq!(untouched, [1.0; 4]);
    }

    #[test]
    fn ramps_start_and_end_silent_and_reach_full_level_past_their_length() {
        let fades = Fades::new(4, 2);
        let gains: Vec<f32> = (0..10).map(|index| fades.gain(index, 10)).collect();
        assert_eq!(gains, [0.0, 0.25, 0.5, 0.75, 1.0, 1.0, 1.0, 1.0, 0.5, 0.0]);
        assert_eq!(fades.gain(10, 10), 0.0);
        assert_eq!(Fades::NONE.gain(0, 10), 1.0);
        assert_eq!(Fades::NONE.gain(9, 10), 1.0);
    }

    #[test]
    fn ramps_that_do_not_fit_are_shortened_with_the_fade_in_kept_first() {
        assert_eq!(Fades::new(6, 6).fitted(10), Fades::new(6, 4));
        assert_eq!(Fades::new(20, 3).fitted(10), Fades::new(10, 0));
        assert_eq!(Fades::new(3, 20).fitted(10), Fades::new(3, 7));
        assert_eq!(Fades::new(3, 4).fitted(10), Fades::new(3, 4));
        assert_eq!(Fades::new(3, 4).fitted(0), Fades::NONE);
    }

    #[test]
    fn applying_a_piece_at_a_time_matches_the_gain_of_every_frame() {
        let fades = Fades::new(5, 3);
        let length = 12;
        let mut whole = vec![1.0f32; 12 * 2];
        fades.apply(&mut whole, 2, 0, length);
        let expected: Vec<f32> = (0..length).flat_map(|index| [fades.gain(index, length); 2]).collect();
        assert_eq!(whole, expected);

        let mut pieces = vec![1.0f32; 12 * 2];
        for (piece, first) in pieces.chunks_mut(2 * 5).zip((0..).step_by(5)) {
            fades.apply(piece, 2, first, length);
        }
        assert_eq!(pieces, expected);
    }

    #[test]
    fn overlapping_ramps_never_apply_twice_to_one_frame() {
        // Fitted, a 6 + 6 ramp on 10 frames becomes 6 + 4, so frame 6 starts the fade out.
        let fades = Fades::new(6, 6);
        let mut samples = vec![1.0f32; 10];
        fades.apply(&mut samples, 1, 0, 10);
        let expected: Vec<f32> = (0..10).map(|index| fades.gain(index, 10)).collect();
        assert_eq!(samples, expected);
        assert_eq!(samples[9], 0.0);
        assert_eq!(samples[0], 0.0);
    }
}
