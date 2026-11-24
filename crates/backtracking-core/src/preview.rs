//! Plays a selection back: the frozen audio of 1 to [`crate::MAX_LANES`] lanes, placed the way
//! their WAVs are exported, mixed, and faded the way the selection is, a block at a time.
//!
//! The editor builds a [`Preview`] and shares it with whatever plays it: a device thread on
//! the desktop or the plug-in's audio thread. Rendering never allocates, locks or frees, so
//! it is safe on a real-time thread. The editor reads the position back to draw a playhead,
//! changes the fades while it plays and asks it to stop.

mod gate;
mod selected;
mod slot;
pub use selected::PreviewLane;
use selected::WideRender;

use std::ops::Range;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::{ChannelRoute, Fades, FrozenTrack, Snapshot};

pub use gate::PreviewGate;
pub use slot::{PreviewSlot, SlotRender};

/// Frames read from the frozen pages per step. Two of these blocks live on the stack.
const BLOCK: usize = 256;
/// A preview stopped part way falls silent over this long instead of cutting off with a click.
const STOP_RAMP_MS: u64 = 8;
const NOT_STOPPED: u64 = u64::MAX;

/// Where one lane's frames land in the stereo preview, following the lane's exported WAV.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Placement {
    Wide(usize),
    Channel(usize, bool),
    /// A stereo WAV.
    Stereo,
    /// A mono WAV, heard from the middle.
    Centre,
    /// A stereo lane averaged into a mono WAV.
    Downmix,
    /// A routed lane written to the left channel of a stereo WAV.
    Left,
    /// A routed lane written to the right channel of a stereo WAV.
    Right,
    /// A side lane written as opposite channels of a stereo WAV.
    Side,
}

impl Placement {
    /// The same choice the WAV writer makes for `route`, or `None` where it would refuse.
    fn new(route: ChannelRoute, channels: usize, force_mono: bool) -> Option<Self> {
        Some(match (route, channels, force_mono) {
            (ChannelRoute::Merged, 2, true) => Self::Downmix,
            (ChannelRoute::Merged, 2, false) => Self::Stereo,
            (ChannelRoute::Left, 1, false) => Self::Left,
            (ChannelRoute::Right, 1, false) => Self::Right,
            (ChannelRoute::Side, 1, false) => Self::Side,
            (_, 1, _) => Self::Centre,
            _ => return None,
        })
    }

    fn place(self, frame: &[f32]) -> [f32; 2] {
        match self {
            Self::Wide(_) => unreachable!("wide lanes are rendered in blocks"),
            Self::Channel(index, true) => [frame[index]; 2],
            Self::Channel(0, false) => [frame[0], 0.],
            Self::Channel(_, false) => [0., frame[1]],
            Self::Stereo => [frame[0], frame[1]],
            Self::Centre => [frame[0], frame[0]],
            Self::Downmix => {
                let middle = (frame[0] + frame[1]) * 0.5;
                [middle, middle]
            }
            Self::Left => [frame[0], 0.0],
            Self::Right => [0.0, frame[0]],
            Self::Side => [frame[0], -frame[0]],
        }
    }
}

/// Where one lane's frames are read from.
enum Source {
    /// A lane of the editor's frozen capture, whose pages the preview shares.
    Track(FrozenTrack),
    /// A snapshot the macOS app owns, read in place. See [`Preview::from_snapshots`].
    Snapshot(SnapshotRef),
}

/// A snapshot that a preview reads without owning it.
struct SnapshotRef(NonNull<Snapshot>);

// SAFETY: a preview only reads the snapshot, `Snapshot` itself is `Send` and `Sync`, and whoever
// built the preview keeps the snapshot alive and unchanged until the preview is dropped.
unsafe impl Send for SnapshotRef {}
unsafe impl Sync for SnapshotRef {}

impl Source {
    fn sample_rate(&self) -> u32 {
        match self {
            Self::Track(track) => track.sample_rate(),
            Self::Snapshot(snapshot) => snapshot.get().sample_rate,
        }
    }

    fn channels(&self) -> usize {
        match self {
            Self::Track(track) => track.channels(),
            Self::Snapshot(snapshot) => snapshot.get().channels,
        }
    }

    fn frame_range(&self) -> Range<u64> {
        match self {
            Self::Track(track) => track.frame_range(),
            Self::Snapshot(snapshot) => snapshot.get().frame_range(),
        }
    }

    /// Copies whole frames starting at timeline frame `first` into `output`, or returns false
    /// when the lane does not hold all of them.
    fn read_frames(&self, first: u64, output: &mut [f32]) -> bool {
        match self {
            Self::Track(track) => track.read_frames(first, output),
            Self::Snapshot(snapshot) => {
                let snapshot = snapshot.get();
                let channels = snapshot.channels;
                if channels == 0 || output.len() % channels != 0 {
                    return false;
                }
                let held = first
                    .checked_sub(snapshot.first_frame)
                    .and_then(|offset| usize::try_from(offset).ok())
                    .and_then(|offset| Some(offset..offset.checked_add(output.len() / channels)?))
                    .and_then(|frames| snapshot.frames(frames));
                match held {
                    Some(samples) => {
                        output.copy_from_slice(samples);
                        true
                    }
                    None => false,
                }
            }
        }
    }
}

impl SnapshotRef {
    fn get(&self) -> &Snapshot {
        // SAFETY: see the `Send` and `Sync` implementations above.
        unsafe { self.0.as_ref() }
    }
}

/// A selection being played back. See the module documentation.
pub struct Preview {
    lanes: Vec<(Source, Placement)>,
    wide: Option<WideRender>,
    native_layout: Option<u64>,
    /// The selection on the retained-audio timeline.
    frames: Range<u64>,
    sample_rate: u32,
    /// The clip frame the next render starts with.
    next: AtomicU64,
    fade_in: AtomicU64,
    fade_out: AtomicU64,
    stop_requested: AtomicBool,
    /// The clip frame the stop ramp began at. Only the rendering thread writes it.
    stopped_at: AtomicU64,
    finished: AtomicBool,
}

impl Preview {
    /// A preview of `frames` of each of `tracks`, routed as their WAVs are, or `None` when the
    /// lanes do not all hold the selection at one sample rate.
    pub fn new(
        tracks: &[FrozenTrack],
        frames: Range<u64>,
        routes: &[ChannelRoute],
        force_mono: bool,
        fades: Fades,
    ) -> Option<Self> {
        let sources = tracks.iter().cloned().map(Source::Track).collect();
        Self::from_sources(sources, frames, routes, force_mono, fades)
    }

    /// Like [`Preview::new`] for the snapshots the macOS app edits, which it reads in place.
    ///
    /// # Safety
    ///
    /// Every snapshot must stay alive, and unchanged, until the preview is dropped. The preview
    /// does not borrow them, so nothing checks this.
    pub(crate) unsafe fn from_snapshots(
        snapshots: &[&Snapshot],
        frames: Range<u64>,
        routes: &[ChannelRoute],
        force_mono: bool,
        fades: Fades,
    ) -> Option<Self> {
        let sources = snapshots
            .iter()
            .map(|&snapshot| Source::Snapshot(SnapshotRef(NonNull::from(snapshot))))
            .collect();
        Self::from_sources(sources, frames, routes, force_mono, fades)
    }

    fn from_sources(
        sources: Vec<Source>,
        frames: Range<u64>,
        routes: &[ChannelRoute],
        force_mono: bool,
        fades: Fades,
    ) -> Option<Self> {
        if routes.len() < sources.len() {
            return None;
        }
        let lanes = sources
            .into_iter()
            .zip(routes)
            .map(|(source, &route)| {
                let placement = Placement::new(route, source.channels(), force_mono)?;
                Some((source, placement))
            })
            .collect::<Option<Vec<_>>>()?;
        Self::prepared(lanes, None, frames, fades)
    }

    fn prepared(
        lanes: Vec<(Source, Placement)>,
        wide: Option<WideRender>,
        frames: Range<u64>,
        fades: Fades,
    ) -> Option<Self> {
        let first = &lanes.first()?.0;
        let sample_rate = first.sample_rate();
        let held = first.frame_range();
        if lanes.len() > crate::MAX_LANES
            || frames.is_empty()
            || sample_rate == 0
            || frames.start < held.start
            || frames.end > held.end
            || lanes
                .iter()
                .any(|(s, _)| s.sample_rate() != sample_rate || s.frame_range() != held)
        {
            return None;
        }
        let length = frames.end - frames.start;
        let fades = fades.fitted(length);
        Some(Self {
            lanes,
            wide,
            native_layout: None,
            frames,
            sample_rate,
            next: AtomicU64::new(0),
            fade_in: AtomicU64::new(fades.fade_in),
            fade_out: AtomicU64::new(fades.fade_out),
            stop_requested: AtomicBool::new(false),
            stopped_at: AtomicU64::new(NOT_STOPPED),
            finished: AtomicBool::new(false),
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// The selection being played, on the retained-audio timeline.
    pub fn frames(&self) -> Range<u64> {
        self.frames.clone()
    }

    pub fn len(&self) -> u64 {
        self.frames.end - self.frames.start
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// How many frames have been rendered, counted from the selection's start. What is heard
    /// trails this by whatever the player still holds in its buffers.
    pub fn rendered(&self) -> u64 {
        self.next.load(Ordering::Acquire)
    }

    pub fn fades(&self) -> Fades {
        Fades::new(self.fade_in.load(Ordering::Relaxed), self.fade_out.load(Ordering::Relaxed))
    }

    /// Changes the ramps from the next rendered block on.
    pub fn set_fades(&self, fades: Fades) {
        let fades = fades.fitted(self.len());
        self.fade_in.store(fades.fade_in, Ordering::Relaxed);
        self.fade_out.store(fades.fade_out, Ordering::Relaxed);
    }

    /// Asks the preview to fall silent. It finishes after a short ramp.
    pub fn stop(&self) {
        self.stop_requested.store(true, Ordering::Release);
    }

    pub fn stop_requested(&self) -> bool {
        self.stop_requested.load(Ordering::Acquire)
    }

    /// Whether every frame the preview will ever produce has been rendered.
    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    /// How many frames are still to be rendered, counting the stop ramp once a stop has been
    /// asked for.
    pub fn remaining(&self) -> u64 {
        let position = self.next.load(Ordering::Acquire);
        let end = match self.stopped_at.load(Ordering::Relaxed) {
            NOT_STOPPED if self.stop_requested() => position.saturating_add(self.ramp()),
            NOT_STOPPED => self.len(),
            at => at.saturating_add(self.ramp()),
        };
        end.min(self.len()).saturating_sub(position)
    }

    fn ramp(&self) -> u64 {
        (u64::from(self.sample_rate) * STOP_RAMP_MS / 1_000).max(1)
    }

    /// Renders up to `frames` stereo frames and hands each to `emit` with its index. Returns how
    /// many it rendered: fewer than asked once the selection or the stop ramp runs out.
    /// One thread renders a preview; others only read its position or change its settings.
    pub fn render_with(&self, frames: usize, mut emit: impl FnMut(usize, [f32; 2])) -> usize {
        self.render_frames(frames, false, |index, pair, _, gain| {
            emit(index, [pair[0] * gain, pair[1] * gain]);
        })
    }

    /// Builds a preview of a complete wide main track and optional narrow companions.
    /// On a matching bus the main plays in channel order. Other outputs get the pair
    /// prepared with `model`, just as in [`Preview::with_channels`]. The caller supplies
    /// the same layout identifier for equivalent speaker orders (zero for an unnamed bus).
    pub fn for_bus(
        lanes: &[PreviewLane<'_>],
        frames: Range<u64>,
        layout: u64,
        model: Option<std::sync::Arc<crate::spatial::HeadModel>>,
        force_mono: bool,
        fades: Fades,
    ) -> Option<Self> {
        let main = lanes.first()?;
        let width = main.track.channels();
        if width <= 2
            || main.selected.is_some()
            || main.group?.channels() != (0..width)
            || lanes[1..].iter().any(|side| side.track.channels() > 2)
        {
            return None;
        }
        let mut preview = Self::with_channels(lanes, frames, model, force_mono, fades)?;
        preview.native_layout = Some(layout);
        Some(preview)
    }

    /// Adds preview samples via `emit(frame, channel, sample)`. The caller mixes these
    /// into its output. Mono hears the average of the pair. An empty bus consumes no frames.
    pub fn render_channels_with(
        &self,
        frames: usize,
        channels: usize,
        layout: u64,
        mut emit: impl FnMut(usize, usize, f32),
    ) -> usize {
        if channels == 0 {
            return 0;
        }
        let native = self.native_layout == Some(layout) && channels == self.lanes[0].0.channels();
        self.render_frames(frames, native, |index, [left, right], main, gain| {
            if !main.is_empty() {
                emit(index, 0, (main[0] + left) * gain);
                emit(index, 1, (main[1] + right) * gain);
                for (channel, sample) in main.iter().enumerate().skip(2) {
                    emit(index, channel, sample * gain);
                }
            } else if channels == 1 {
                emit(index, 0, (left * gain + right * gain) * 0.5);
            } else {
                emit(index, 0, left * gain);
                emit(index, 1, right * gain);
            }
        })
    }

    fn render_frames(
        &self,
        frames: usize,
        native: bool,
        emit: impl FnMut(usize, [f32; 2], &[f32], f32),
    ) -> usize {
        match &self.wide {
            Some(wide) => wide.render(|lanes| self.render_blocks(frames, native, emit, lanes)),
            None => self.render_blocks(frames, false, emit, &mut []),
        }
    }

    fn render_blocks(
        &self,
        frames: usize,
        native: bool,
        mut emit: impl FnMut(usize, [f32; 2], &[f32], f32),
        wide: &mut [selected::WideLane],
    ) -> usize {
        let length = self.len();
        let mut position = self.next.load(Ordering::Relaxed);
        let fades = self.fades().fitted(length);
        let ramp = self.ramp();
        let stopped_at = if self.stop_requested.load(Ordering::Acquire) {
            let at = self.stopped_at.load(Ordering::Relaxed);
            if at == NOT_STOPPED {
                self.stopped_at.store(position, Ordering::Relaxed);
                position
            } else {
                at
            }
        } else {
            NOT_STOPPED
        };
        let end = length.min(stopped_at.saturating_add(ramp));
        let mut written = 0;
        while written < frames && position < end {
            let count = (frames - written).min(BLOCK).min((end - position) as usize);
            let mut mixed = [[0.0f32; 2]; BLOCK];
            for (source, placement) in &self.lanes {
                if let Placement::Wide(index) = placement {
                    if native {
                        wide[*index].read(source, self.frames.start + position, count);
                    } else {
                        wide[*index].mix(source, self.frames.start + position, &mut mixed[..count]);
                    }
                    continue;
                }
                let mut read = [0.0f32; BLOCK * 2];
                // A placement exists only for one or two channels.
                let channels = source.channels();
                let samples = &mut read[..count * channels];
                if !source.read_frames(self.frames.start + position, samples) {
                    samples.fill(0.0);
                }
                for (mixed, frame) in mixed.iter_mut().zip(samples.chunks_exact(channels)) {
                    let [left, right] = placement.place(frame);
                    mixed[0] += left;
                    mixed[1] += right;
                }
            }
            for (offset, [left, right]) in mixed[..count].iter().copied().enumerate() {
                let index = position + offset as u64;
                let mut gain = fades.gain(index, length);
                if index >= stopped_at {
                    gain *= (stopped_at + ramp - 1 - index) as f32 / ramp as f32;
                }
                let main = if native {
                    wide[0].frame(offset, self.lanes[0].0.channels())
                } else {
                    &[]
                };
                emit(written + offset, [left, right], main, gain);
            }
            position += count as u64;
            written += count;
        }
        self.next.store(position, Ordering::Release);
        if position >= end {
            self.finished.store(true, Ordering::Release);
        }
        written
    }

    /// Renders into `output`, interleaved frames of `channels` samples, and silences whatever it
    /// could not fill. One output channel hears both sides; channels past the second stay silent.
    pub fn render_interleaved(&self, output: &mut [f32], channels: usize) -> usize {
        if channels == 0 {
            return 0;
        }
        let frames = output.len() / channels;
        let written = self.render_with(frames, |index, [left, right]| {
            let frame = &mut output[index * channels..(index + 1) * channels];
            if let [only] = frame {
                *only = (left + right) * 0.5;
            } else {
                frame[0] = left;
                frame[1] = right;
                frame[2..].fill(0.0);
            }
        });
        output[written * channels..].fill(0.0);
        written
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp_track(first_frame: u64, frames: usize, channels: usize) -> FrozenTrack {
        let samples: Vec<f32> = (0..frames * channels)
            .map(|index| (index / channels) as f32 + if index % channels == 1 { 0.5 } else { 0.0 })
            .collect();
        FrozenTrack::from_interleaved(48_000, channels, first_frame, &samples).unwrap()
    }

    fn rendered(preview: &Preview, block: usize) -> Vec<[f32; 2]> {
        let mut frames = Vec::new();
        loop {
            let before = frames.len();
            let count = preview.render_with(block, |index, frame| {
                assert_eq!(index, frames.len() - before);
                frames.push(frame);
            });
            if count == 0 {
                return frames;
            }
        }
    }

    #[test]
    fn a_preview_plays_exactly_the_selected_frames_from_its_left_edge() {
        let track = ramp_track(1_000, 500, 2);
        let preview = Preview::new(&[track], 1_100..1_400, &[ChannelRoute::Merged], false,
            Fades::NONE).unwrap();
        let frames = rendered(&preview, 70);
        assert_eq!(frames.len(), 300);
        assert_eq!(frames[0], [100.0, 100.5]);
        assert_eq!(frames[299], [399.0, 399.5]);
        assert!(preview.is_finished());
        assert_eq!(preview.rendered(), 300);
    }

    #[test]
    fn lanes_are_placed_as_their_wavs_and_summed() {
        let a = ramp_track(0, 10, 1);
        let b = ramp_track(0, 10, 1);
        let place = |routes: [ChannelRoute; 2], force_mono| {
            let preview = Preview::new(&[a.clone(), b.clone()], 4..5, &routes, force_mono,
                Fades::NONE).unwrap();
            rendered(&preview, 8)[0]
        };
        assert_eq!(place([ChannelRoute::Left, ChannelRoute::Right], false), [4.0, 4.0]);
        assert_eq!(place([ChannelRoute::Left, ChannelRoute::Left], false), [8.0, 0.0]);
        assert_eq!(place([ChannelRoute::Side, ChannelRoute::Center], false), [8.0, 0.0]);
        // Forced mono writes routed lanes as mono files, heard from the middle.
        assert_eq!(place([ChannelRoute::Left, ChannelRoute::Right], true), [8.0, 8.0]);

        let stereo = ramp_track(0, 10, 2);
        let preview = Preview::new(&[stereo], 2..3, &[ChannelRoute::Merged], true, Fades::NONE)
            .unwrap();
        assert_eq!(rendered(&preview, 8)[0], [2.25, 2.25]);
    }

    #[test]
    fn three_or_more_lanes_are_placed_as_their_wavs_and_summed() {
        let a = ramp_track(0, 10, 2);
        let b = ramp_track(0, 10, 1);
        let c = ramp_track(0, 10, 1);
        let routes = [ChannelRoute::Merged, ChannelRoute::Left, ChannelRoute::Right];
        let preview = Preview::new(&[a.clone(), b.clone(), c], 4..6, &routes, false,
            Fades::NONE).unwrap();
        // A stereo lane, a lane on the left and one on the right, each at its frame number.
        assert_eq!(rendered(&preview, 8), [[8.0, 8.5], [10.0, 10.5]]);
        // As many lanes as a capture holds mix the same way, and one more is refused.
        let quarter = FrozenTrack::from_interleaved(48_000, 1, 0, &[0.25; 10]).unwrap();
        let all = vec![quarter.clone(); crate::MAX_LANES];
        let merged = [ChannelRoute::Merged; crate::MAX_LANES + 1];
        let preview = Preview::new(&all, 0..10, &merged[..crate::MAX_LANES], false, Fades::NONE)
            .unwrap();
        assert_eq!(rendered(&preview, 4), [[16.0, 16.0]; 10]);
        let too_many = vec![quarter; crate::MAX_LANES + 1];
        assert!(Preview::new(&too_many, 0..10, &merged, false, Fades::NONE).is_none());
        // A third lane must hold the selection at the same rate, as a second must.
        let short = ramp_track(0, 5, 1);
        assert!(Preview::new(&[a.clone(), b.clone(), short], 4..6, &routes, false,
            Fades::NONE).is_none());
        let other_rate = FrozenTrack::from_interleaved(44_100, 1, 0, &[0.0; 10]).unwrap();
        assert!(Preview::new(&[a, b, other_rate], 4..6, &routes, false, Fades::NONE).is_none());
    }

    #[test]
    fn lanes_that_do_not_hold_the_selection_at_one_rate_are_refused() {
        let held = ramp_track(100, 100, 2);
        let route = [ChannelRoute::Merged];
        assert!(Preview::new(&[held.clone()], 90..150, &route, false, Fades::NONE).is_none());
        assert!(Preview::new(&[held.clone()], 150..201, &route, false, Fades::NONE).is_none());
        assert!(Preview::new(&[held.clone()], 150..150, &route, false, Fades::NONE).is_none());
        assert!(Preview::new(&[held.clone()], 150..200, &[], false, Fades::NONE).is_none());
        // A stereo lane cannot be routed to one side.
        assert!(Preview::new(&[held.clone()], 150..200, &[ChannelRoute::Left], false,
            Fades::NONE).is_none());
        let other_rate = FrozenTrack::from_interleaved(44_100, 2, 100, &[0.0; 200]).unwrap();
        assert!(Preview::new(&[held, other_rate], 150..200, &[ChannelRoute::Merged; 2], false,
            Fades::NONE).is_none());
    }

    #[test]
    fn fades_shape_the_preview_and_can_change_while_it_plays() {
        let ones = FrozenTrack::from_interleaved(48_000, 1, 0, &[1.0; 100]).unwrap();
        let preview = Preview::new(&[ones], 0..100, &[ChannelRoute::Merged], false,
            Fades::new(10, 20)).unwrap();
        let mut first = Vec::new();
        preview.render_with(50, |_, [left, _]| first.push(left));
        assert_eq!(first[0], 0.0);
        assert_eq!(first[5], 0.5);
        assert_eq!(first[10], 1.0);
        preview.set_fades(Fades::new(10, 50));
        let mut rest = Vec::new();
        preview.render_with(50, |_, [left, _]| rest.push(left));
        assert_eq!(rest[0], Fades::new(10, 50).gain(50, 100));
        assert_eq!(rest[49], 0.0);
        assert_eq!(preview.fades(), Fades::new(10, 50));
        // Ramps that do not fit are shortened.
        preview.set_fades(Fades::new(90, 90));
        assert_eq!(preview.fades(), Fades::new(90, 10));
    }

    #[test]
    fn stopping_ramps_down_from_where_it_was_and_then_finishes() {
        let ones = FrozenTrack::from_interleaved(48_000, 1, 0, &[1.0; 48_000]).unwrap();
        let preview = Preview::new(&[ones], 0..48_000, &[ChannelRoute::Merged], false,
            Fades::NONE).unwrap();
        assert_eq!(preview.render_with(1_000, |_, _| {}), 1_000);
        preview.stop();
        let mut tail = Vec::new();
        while preview.render_with(100, |_, [left, _]| tail.push(left)) != 0 {}
        // 8 ms at 48 kHz.
        assert_eq!(tail.len(), 384);
        assert!(tail[0] < 1.0 && tail[0] > 0.99);
        assert!(tail.windows(2).all(|pair| pair[1] < pair[0]));
        assert_eq!(*tail.last().unwrap(), 0.0);
        assert!(preview.is_finished());
        assert_eq!(preview.rendered(), 1_384);
    }

    #[test]
    fn snapshots_read_in_place_play_like_the_frozen_tracks_they_copy() {
        let interleaved: Vec<f32> = (0..200).map(|index| index as f32).collect();
        let stereo = Snapshot { sample_rate: 48_000, channels: 2, samples: interleaved.clone(),
            first_frame: 500 };
        let mono = Snapshot { sample_rate: 48_000, channels: 1, samples: interleaved[..100].to_vec(),
            first_frame: 500 };
        let tracks = [
            FrozenTrack::from_interleaved(48_000, 2, 500, &interleaved).unwrap(),
            FrozenTrack::from_interleaved(48_000, 1, 500, &interleaved[..100]).unwrap(),
        ];
        let routes = [ChannelRoute::Merged, ChannelRoute::Right];
        let fades = Fades::new(7, 11);
        let expected = rendered(&Preview::new(&tracks, 530..590, &routes, false, fades).unwrap(), 16);
        // SAFETY: the snapshots outlive the previews built from them in this test.
        let preview = unsafe { Preview::from_snapshots(&[&stereo, &mono], 530..590, &routes, false,
            fades) }.unwrap();
        assert_eq!(rendered(&preview, 16), expected);
        assert_eq!(expected.len(), 60);

        // A selection the snapshots do not wholly hold is refused, as it is for tracks, and so is
        // a pair whose lanes hold different frames, whose WAVs could not be exported.
        let refused = |snapshots: &[&Snapshot], frames: Range<u64>| unsafe {
            Preview::from_snapshots(snapshots, frames, &routes, false, Fades::NONE)
        }.is_none();
        // Both snapshots hold frames 500..600.
        assert!(refused(&[&stereo, &mono], 490..590));
        assert!(refused(&[&stereo, &mono], 530..601));
        assert!(!refused(&[&stereo, &mono], 500..600));
        let later = Snapshot { sample_rate: 48_000, channels: 1,
            samples: interleaved[..100].to_vec(), first_frame: 520 };
        assert!(refused(&[&stereo, &later], 530..590));
        assert!(!refused(&[&later], 530..590));
        let silent = Snapshot { sample_rate: 0, channels: 1, samples: vec![0.0; 10], first_frame: 0 };
        assert!(unsafe { Preview::from_snapshots(&[&silent], 0..5, &routes, false, Fades::NONE) }
            .is_none());
    }

    #[test]
    fn interleaved_output_fills_every_channel_layout_and_silences_the_rest() {
        let track = ramp_track(0, 4, 2);
        let preview = || Preview::new(&[track.clone()], 1..3, &[ChannelRoute::Merged], false,
            Fades::NONE).unwrap();
        let mut stereo = [9.0f32; 6];
        assert_eq!(preview().render_interleaved(&mut stereo, 2), 2);
        assert_eq!(stereo, [1.0, 1.5, 2.0, 2.5, 0.0, 0.0]);
        let mut mono = [9.0f32; 3];
        assert_eq!(preview().render_interleaved(&mut mono, 1), 2);
        assert_eq!(mono, [1.25, 2.25, 0.0]);
        let mut quad = [9.0f32; 8];
        assert_eq!(preview().render_interleaved(&mut quad, 4), 2);
        assert_eq!(quad, [1.0, 1.5, 0.0, 0.0, 2.0, 2.5, 0.0, 0.0]);
    }
}
