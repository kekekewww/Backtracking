use std::ops::Range;

use super::{PairGroup, PairSide, WaveformView};

/// How many frequency bands a [`SpectrumBin`] holds, lowest first.
pub const SPECTRUM_BANDS: usize = 6;

/// The band edges in Hz, lowest first: band `k` spans `EDGES[k]..EDGES[k + 1]`.
///
/// The inner four bands are about 1.4 octaves wide, the outer two wider: a kick or bass line
/// sits in band 0 (up to 120 Hz), voice and guitar in bands 2 and 3, cymbals and hi-hats in
/// band 5 (from 5.6 kHz).
/// Audio below 20 Hz is left out. Above 48 kHz the top band stops at 20 kHz; at 48 kHz and
/// below it reaches the Nyquist frequency. A band whose lower edge is within half an octave
/// of the Nyquist frequency is left empty, and the band below it reaches the Nyquist
/// frequency instead: at 8 kHz band 5 is empty and band 4 spans 2.2 to 4 kHz.
pub const SPECTRUM_BAND_EDGES_HZ: [f32; SPECTRUM_BANDS + 1] =
    [20.0, 120.0, 320.0, 850.0, 2_200.0, 5_600.0, 20_000.0];

/// One bin's tilted band energies, lowest band first. Each is a mean square per frame,
/// summed over the row's channels, multiplied by a fixed per-band tilt that gives pink noise
/// equal values in every band the sample rate can hold. Only their ratios carry meaning.
pub type BandEnergies = [f32; SPECTRUM_BANDS];

/// One spectrum bin: `None` where there is no audio to describe, meaning frames outside the
/// held range or a mean square below -90 dBFS (silence, including known silence).
pub type SpectrumBin = Option<BandEnergies>;

/// Which waveform row a spectrum is read for. Each matches the row the waveform draws from the
/// same lane: [`FrozenTrack::waveform`](crate::FrozenTrack::waveform) for `View`,
/// [`channel_waveform`](crate::FrozenTrack::channel_waveform) for `Channel` and
/// [`pair_waveform`](crate::FrozenTrack::pair_waveform) for `Pair`.
///
/// A row of several channels sums their band energies instead of analysing their mix:
/// `Merged` sums every lane channel, a pair side weights each channel of the group by the
/// square of its fold-down gain to that side.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SpectrumRow<'a> {
    View(WaveformView),
    /// A zero-based channel within the lane.
    Channel(usize),
    Pair(&'a PairGroup, PairSide),
}

impl SpectrumRow<'_> {
    /// The frame channels of a lane occupying `track` that this row sums, each with its
    /// power weight, or `None` when the lane has no such row.
    pub(crate) fn members(&self, track: Range<usize>) -> Option<Vec<(usize, f32)>> {
        let width = track.len();
        if width == 0 {
            return None;
        }
        Some(match *self {
            Self::View(WaveformView::Merged) => track.map(|channel| (channel, 1.0)).collect(),
            Self::View(WaveformView::Left) => vec![(track.start, 1.0)],
            Self::View(WaveformView::Right) => vec![(track.start + usize::from(width > 1), 1.0)],
            Self::Channel(channel) if channel < width => vec![(track.start + channel, 1.0)],
            Self::Channel(_) => return None,
            Self::Pair(group, side) => {
                if width <= 2 || group.channels().end > width {
                    return None;
                }
                group
                    .weights
                    .iter()
                    .enumerate()
                    .map(|(index, weights)| {
                        (
                            track.start + group.first + index,
                            weights[side as usize].powi(2),
                        )
                    })
                    .filter(|&(_, weight)| weight > 0.0)
                    .collect()
            }
        })
    }
}
