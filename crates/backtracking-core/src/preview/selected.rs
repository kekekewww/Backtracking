use super::*;
use crate::{
    spatial::{self, BinauralRenderer, BinauralSource, HeadModel},
    waveform::PairGroup,
};
use std::{cell::UnsafeCell, sync::Arc};

/// Native channel indices are relative to this track. None selects the whole Channels group.
pub struct PreviewLane<'a> {
    pub track: &'a FrozenTrack,
    pub route: ChannelRoute,
    pub group: Option<&'a PairGroup>,
    pub selected: Option<&'a [usize]>,
}

impl Preview {
    /// Construct on the editor thread. Supply a prepared head model for binaural playback,
    /// or None for stereo. Narrow lanes keep their ordinary routing.
    pub fn with_channels(
        lanes: &[PreviewLane<'_>],
        frames: Range<u64>,
        model: Option<Arc<HeadModel>>,
        force_mono: bool,
        fades: Fades,
    ) -> Option<Self> {
        Self::with_positions(lanes, frames, model, force_mono, fades, &[None, None])
    }

    /// Each group is the group being auditioned. Frozen tracks use lane-native indices.
    pub fn with_positions(
        lanes: &[PreviewLane<'_>],
        frames: Range<u64>,
        model: Option<Arc<HeadModel>>,
        force_mono: bool,
        fades: Fades,
        positions: &[Option<&crate::FrozenPositionTrack>; 2],
    ) -> Option<Self> {
        if lanes.is_empty() || lanes.len() > 2 {
            return None;
        }
        let mut sources = Vec::new();
        let mut wide = Vec::new();
        for (lane_index, lane) in lanes.iter().enumerate() {
            let width = lane.track.channels();
            let placement = if width > 2 {
                let index = wide.len();
                wide.push(WideLane::new(
                    lane,
                    model.as_ref(),
                    frames.clone(),
                    positions[lane_index],
                )?);
                Placement::Wide(index)
            } else {
                match lane.selected {
                    Some(&[c]) if width == 2 && c < 2 => Placement::Channel(c, force_mono),
                    None | Some(&[0]) if width == 1 => {
                        Placement::new(lane.route, width, force_mono)?
                    }
                    None | Some(&[0, 1] | &[1, 0]) if width == 2 => {
                        Placement::new(lane.route, width, force_mono)?
                    }
                    _ => return None,
                }
            };
            sources.push((Source::Track(lane.track.clone()), placement));
        }
        let wide = (!wide.is_empty()).then(|| WideRender {
            active: AtomicBool::new(false),
            lanes: UnsafeCell::new(wide),
        });
        Self::prepared(sources, wide, frames, fades)
    }
}

impl crate::FrozenPositionTrack {
    /// Absolute renderer keys, including one neighbor beyond each end for interpolation.
    pub fn render_keys(&self, channel: usize, frames: Range<u64>) -> Vec<spatial::PositionKey> {
        if frames.is_empty() {
            return Vec::new();
        }
        let mut keys = self.channel_positions(channel, frames.start..u64::MAX);
        let count = keys.partition_point(|k| k.frame < frames.end);
        if count == 0 {
            return Vec::new();
        }
        keys.truncate(count + 1);
        keys.into_iter()
            .map(|key| {
                let p = key.position;
                spatial::PositionKey {
                    frame: key.frame,
                    x: p.x as f32,
                    y: p.y as f32,
                    z: p.z as f32,
                    size: p.size as f32,
                    gain: p.gain.clamp(0., 4.) as f32,
                }
            })
            .collect()
    }
}

enum Render {
    Stereo(Vec<[f32; 2]>),
    Binaural(BinauralRenderer),
}

pub(super) struct WideLane {
    range: Range<usize>,
    input: Vec<f32>,
    render: Render,
}

impl WideLane {
    fn new(
        lane: &PreviewLane<'_>,
        model: Option<&Arc<HeadModel>>,
        frames: Range<u64>,
        positions: Option<&crate::FrozenPositionTrack>,
    ) -> Option<Self> {
        let group = lane.group?;
        let range = group.channels();
        if range.end > lane.track.channels() {
            return None;
        }
        let selected = lane.selected.unwrap_or(&[]);
        if selected
            .iter()
            .enumerate()
            .any(|(i, c)| !range.contains(c) || selected[..i].contains(c))
        {
            return None;
        }
        let selected = lane
            .selected
            .map(|channels| channels.iter().map(|c| c - range.start).collect::<Vec<_>>());
        let moving = positions.map(|p| {
            range
                .clone()
                .enumerate()
                .map(|(i, channel)| {
                    if selected.as_ref().is_none_or(|indices| indices.contains(&i)) {
                        p.render_keys(channel, frames.clone())
                    } else {
                        Vec::new()
                    }
                })
                .collect::<Vec<_>>()
        });
        let has_positions = moving
            .as_ref()
            .is_some_and(|tracks| tracks.iter().any(|keys| !keys.is_empty()));
        let model = match model {
            Some(model) => Some(Arc::clone(model)),
            None if has_positions => Some(Arc::new(HeadModel::new(lane.track.sample_rate()).ok()?)),
            None => None,
        };
        let render = if let Some(model) = model {
            if model.sample_rate() != lane.track.sample_rate() {
                return None;
            }
            let labels: Vec<_> = group.labels.iter().map(String::as_str).collect();
            let mut tracks = moving.into_iter().flatten();
            let sources: Vec<_> = spatial::speaker_directions(&labels)
                .into_iter()
                .map(|direction| {
                    let keys = tracks.next().unwrap_or_default();
                    if keys.is_empty() {
                        BinauralSource::Fixed(direction)
                    } else {
                        BinauralSource::Moving(keys)
                    }
                })
                .collect();
            Render::Binaural(
                BinauralRenderer::new(model, &sources, selected.as_deref(), frames.start).ok()?,
            )
        } else {
            let mut weights = group.weights.to_vec();
            if let Some(channels) = selected {
                for (i, w) in weights.iter_mut().enumerate() {
                    if !channels.contains(&i) {
                        *w = [0.; 2];
                    }
                }
            }
            Render::Stereo(weights)
        };
        Some(Self {
            range,
            input: vec![0.; lane.track.channels() * BLOCK],
            render,
        })
    }

    pub(super) fn read(&mut self, source: &Source, first: u64, frames: usize) {
        let input = &mut self.input[..frames * source.channels()];
        if !source.read_frames(first, input) {
            input.fill(0.);
        }
    }

    pub(super) fn frame(&self, offset: usize, width: usize) -> &[f32] {
        &self.input[offset * width..(offset + 1) * width]
    }

    pub(super) fn mix(&mut self, source: &Source, first: u64, mixed: &mut [[f32; 2]]) {
        self.read(source, first, mixed.len());
        let width = source.channels();
        let input = &mut self.input[..mixed.len() * width];
        let group = self.range.len();
        if group != width {
            for frame in 0..mixed.len() {
                let start = frame * width + self.range.start;
                input.copy_within(start..start + group, frame * group);
            }
        }
        let input = &input[..mixed.len() * group];
        let mut output = [0.; 2 * BLOCK];
        let output = &mut output[..mixed.len() * 2];
        match &mut self.render {
            Render::Stereo(weights) => spatial::fold_down(input, weights, output),
            Render::Binaural(renderer) => renderer.render(input, output),
        }
        .expect("validated frame range and complete group frames");
        for (frame, pair) in mixed.iter_mut().zip(output.chunks_exact(2)) {
            frame[0] += pair[0];
            frame[1] += pair[1];
        }
    }
}

// The public transport has shared references for UI controls and the existing FFI.
// A single callback owns the FIR histories and scratch. Concurrent or reentrant
// render attempts return immediately, including from a user-supplied emit closure.
pub(super) struct WideRender {
    active: AtomicBool,
    lanes: UnsafeCell<Vec<WideLane>>,
}
// SAFETY: only render accesses lanes. Atomic exchange grants exclusive access,
// and release on scope exit publishes every change to the next renderer.
unsafe impl Sync for WideRender {}
impl WideRender {
    pub(super) fn render(&self, run: impl FnOnce(&mut [WideLane]) -> usize) -> usize {
        if self.active.swap(true, Ordering::Acquire) {
            return 0;
        }
        struct Release<'a>(&'a AtomicBool);
        impl Drop for Release<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let _release = Release(&self.active);
        // SAFETY: the exclusive claim lasts through run, including unwinding.
        run(unsafe { &mut *self.lanes.get() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_main_and_sidechain_follow_the_stereo_gain_at_every_frame() {
        let width = 12;
        let count = 1400;
        let samples: Vec<_> = (0..count * width).map(|i| i as f32 / 32768.0).collect();
        let main = FrozenTrack::from_interleaved(48000, width, 100, &samples).unwrap();
        let side =
            FrozenTrack::from_interleaved(48000, 2, 100, &[0.25, -0.5].repeat(count)).unwrap();
        let ones = FrozenTrack::from_interleaved(48000, 1, 100, &vec![1.0; count]).unwrap();
        let group = PairGroup::new(0, &vec!["unknown".into(); width]).unwrap();
        let lanes = [
            PreviewLane {
                track: &main,
                group: Some(&group),
                route: ChannelRoute::Merged,
                selected: None,
            },
            PreviewLane {
                track: &side,
                group: None,
                route: ChannelRoute::Merged,
                selected: None,
            },
        ];
        let selection = 137..1397;
        let fades = Fades::new(129, 411);
        for block in [1, 67, 256, 513] {
            for stop_at in [800, 1260] {
                let preview =
                    Preview::for_bus(&lanes, selection.clone(), 0xfff, None, false, fades).unwrap();
                let reference = Preview::new(
                    std::slice::from_ref(&ones),
                    selection.clone(),
                    &[ChannelRoute::Merged],
                    false,
                    fades,
                )
                .unwrap();
                let mut pos = 0;
                while !reference.is_finished() {
                    let n = if pos < stop_at {
                        block.min(stop_at - pos)
                    } else {
                        block
                    };
                    let mut gains = vec![];
                    reference.render_with(n, |_, [gain, _]| gains.push(gain));
                    let mut calls = 0;
                    let written = preview.render_channels_with(n, width, 0xfff, |i, c, sample| {
                        let base = samples[(37 + pos + i) * width + c];
                        let extra = match c {
                            0 => 0.25,
                            1 => -0.5,
                            _ => 0.0,
                        };
                        assert_eq!(sample, (base + extra) * gains[i]);
                        calls += 1;
                    });
                    assert_eq!(written, gains.len());
                    assert_eq!(calls, written * width);
                    pos += written;
                    if pos == stop_at {
                        preview.stop();
                        reference.stop();
                    }
                }
                assert!(preview.is_finished());
                assert_eq!(pos, (stop_at + 384).min(1260));
            }
        }
    }

    #[test]
    fn mono_bus_hears_both_ears_and_the_companion_lane() {
        let group = PairGroup::new(0, &["FL".into(), "FR".into(), "FC".into()]).unwrap();
        let main =
            FrozenTrack::from_interleaved(48000, 3, 0, &[0.125, -0.25, 0.5].repeat(512)).unwrap();
        let side = FrozenTrack::from_interleaved(48000, 2, 0, &[0.75, 0.25].repeat(512)).unwrap();
        let lanes = [
            PreviewLane {
                track: &main,
                route: ChannelRoute::Merged,
                group: Some(&group),
                selected: None,
            },
            PreviewLane {
                track: &side,
                route: ChannelRoute::Merged,
                group: None,
                selected: None,
            },
        ];
        let model = Arc::new(HeadModel::new(48000).unwrap());
        let preview =
            Preview::for_bus(&lanes, 0..512, 7, Some(model.clone()), false, Fades::NONE).unwrap();
        let reference =
            Preview::with_channels(&lanes, 0..512, Some(model), false, Fades::NONE).unwrap();
        let mut expected = Vec::new();
        reference.render_with(512, |_, [l, r]| expected.push((l + r) * 0.5));
        let gate = Box::leak(Box::new(PreviewGate::local()));
        let slot = PreviewSlot::with_gate(gate);
        slot.play(preview);
        assert_eq!(
            slot.render_channels_with(512, 44100, 1, 0, |_, _, _| panic!()),
            SlotRender::default()
        );
        let mut mono = Vec::new();
        let played = slot.render_channels_with(600, 48000, 1, 0, |i, c, x| {
            assert_eq!((i, c), (mono.len(), 0));
            mono.push(x);
        });
        assert_eq!(mono, expected);
        assert_eq!(
            played,
            SlotRender {
                frames: 512,
                remaining: 0
            }
        );
        assert_eq!(gate.covered(crate::capture_clock_ns(), 512, 48000), 0..512);
    }

    #[test]
    fn renderer_keys_keep_both_neighbors_and_bound_gain_before_conversion() {
        use crate::{Position, PositionKey, PositionTrack};
        let mut track = PositionTrack::new();
        for (frame, gain) in [(100, -100.), (200, f64::MAX), (300, 2.)] {
            track.insert(
                5,
                PositionKey {
                    frame,
                    position: Position {
                        x: 0.5,
                        y: -1.,
                        z: 0.125,
                        size: 0.25,
                        gain,
                    },
                },
            );
        }
        let track = track.freeze();
        let keys = track.render_keys(5, 130..175);
        assert_eq!(keys.len(), 2);
        assert_eq!(
            keys.iter().map(|k| (k.frame, k.gain)).collect::<Vec<_>>(),
            [(100, 0.), (200, 4.)]
        );
        assert_eq!(
            (keys[1].x, keys[1].y, keys[1].z, keys[1].size),
            (0.5, -1., 0.125, 0.25)
        );
        assert!(track.render_keys(5, 5..100).is_empty());
        assert!(track.render_keys(4, 100..300).is_empty());
    }

    fn group() -> PairGroup {
        PairGroup::new(1, &["FL".into(), "FR".into(), "FC".into(), "LFE".into()]).unwrap()
    }

    #[test]
    fn selected_preview_matches_engine_across_callback_sizes() {
        let raw: Vec<_> = (0..1400 * 6)
            .map(|i| ((i * 37 % 107) as f32 - 53.) / 64.)
            .collect();
        let track = FrozenTrack::from_interleaved(48000, 6, 300, &raw).unwrap();
        let group = group();
        let frames = 347..1609;
        let pcm: Vec<_> = raw[47 * 6..1309 * 6]
            .chunks_exact(6)
            .flat_map(|f| f[1..5].iter().copied())
            .collect();
        let model = Arc::new(HeadModel::new(48000).unwrap());
        let dirs = spatial::speaker_directions(&["FL", "FR", "FC", "LFE"])
            .into_iter()
            .map(BinauralSource::Fixed)
            .collect::<Vec<_>>();
        let fades = Fades::new(29, 73);
        let narrow = FrozenTrack::from_interleaved(48000, 1, 300, &[0.125; 1400]).unwrap();
        for binaural in [false, true] {
            for selected in [None, Some(&[2, 4][..]), Some(&[])] {
                let indices = selected.map(|s| s.iter().map(|c| c - 1).collect::<Vec<_>>());
                let expected = if binaural {
                    spatial::render_binaural(
                        model.clone(),
                        &dirs,
                        indices.as_deref(),
                        frames.start,
                        &pcm,
                    )
                    .unwrap()
                } else {
                    let mut input = pcm.clone();
                    for frame in input.chunks_exact_mut(4) {
                        for (i, x) in frame.iter_mut().enumerate() {
                            if indices.as_ref().is_some_and(|sel| !sel.contains(&i)) {
                                *x = 0.;
                            }
                        }
                    }
                    let mut stereo = vec![0.; input.len() / 2];
                    spatial::fold_down(&input, &group.weights, &mut stereo).unwrap();
                    stereo
                };
                for block in [1, 17, 128, 257, 1024] {
                    let lanes = [
                        PreviewLane {
                            track: &track,
                            route: ChannelRoute::Merged,
                            group: Some(&group),
                            selected,
                        },
                        PreviewLane {
                            track: &narrow,
                            route: ChannelRoute::Right,
                            group: None,
                            selected: None,
                        },
                    ];
                    let preview = Preview::with_channels(
                        &lanes,
                        frames.clone(),
                        binaural.then(|| model.clone()),
                        false,
                        fades,
                    )
                    .unwrap();
                    let mut pos = 0;
                    while preview.render_with(block, |_, pair| {
                        let gain = fades.gain(pos as u64, preview.len());
                        assert_eq!(
                            pair,
                            [
                                expected[2 * pos] * gain,
                                (expected[2 * pos + 1] + 0.125) * gain
                            ]
                        );
                        pos += 1;
                    }) > 0
                    {}
                    assert_eq!(pos * 2, expected.len());
                }
            }
        }
    }

    #[test]
    fn selected_stereo_side_and_wide_stop_use_the_common_transport() {
        let track = FrozenTrack::from_interleaved(48000, 2, 0, &[0.25, -0.5]).unwrap();
        for (c, route) in [(0, ChannelRoute::Left), (1, ChannelRoute::Right)] {
            for mono in [false, true] {
                let sel = [c];
                let lane = PreviewLane {
                    track: &track,
                    route,
                    group: None,
                    selected: Some(&sel),
                };
                let preview =
                    Preview::with_channels(&[lane], 0..1, None, mono, Fades::NONE).unwrap();
                let mut out = [0.; 2];
                preview.render_interleaved(&mut out, 2);
                assert_eq!(
                    out,
                    if mono {
                        [[0.25; 2], [-0.5; 2]][c]
                    } else {
                        [[0.25, 0.], [0., -0.5]][c]
                    }
                );
            }
        }
        let track = FrozenTrack::from_interleaved(48000, 6, 0, &[1.; 6 * 999]).unwrap();
        let group = group();
        let lane = PreviewLane {
            track: &track,
            route: ChannelRoute::Merged,
            group: Some(&group),
            selected: None,
        };
        let preview = Preview::with_channels(&[lane], 0..999, None, false, Fades::NONE).unwrap();
        // A nested render from an emit callback cannot alias the FIR histories or advance the transport.
        assert_eq!(
            preview.render_with(1, |_, _| assert_eq!(
                preview.render_with(3, |_, _| panic!()),
                0
            )),
            1
        );
        let mut steady = [0.; 2];
        preview.render_interleaved(&mut steady, 2);
        preview.stop();
        assert_eq!(
            preview.render_with(999, |i, out| {
                let g = (383 - i) as f32 / 384.;
                assert_eq!(out, [steady[0] * g, steady[1] * g]);
            }),
            384
        );
        assert!(preview.is_finished());
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "release benchmark"]
    fn pair_preview_cost() {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetCurrentThread() -> isize;
            fn GetThreadTimes(
                thread: isize,
                created: *mut u64,
                exit: *mut u64,
                kernel: *mut u64,
                user: *mut u64,
            ) -> i32;
        }
        fn cpu_time() -> f64 {
            let (mut created, mut exit, mut kernel, mut user) = (0, 0, 0, 0);
            // Four writable FILETIME pairs, each eight bytes, for the calling thread.
            assert_ne!(
                unsafe {
                    GetThreadTimes(
                        GetCurrentThread(),
                        &mut created,
                        &mut exit,
                        &mut kernel,
                        &mut user,
                    )
                },
                0
            );
            (kernel + user) as f64 * 1e-7
        }
        let model = Arc::new(HeadModel::new(48000).unwrap());
        for width in [12, 28] {
            let labels: Vec<_> = [
                "FL", "FR", "FC", "LFE", "Ls", "Rs", "Lb", "Rb", "Ltf", "Rtf", "Ltr", "Rtr",
            ]
            .into_iter()
            .cycle()
            .take(width)
            .map(str::to_string)
            .collect();
            let group = PairGroup::new(0, &labels).unwrap();
            let n = 48000 * 4;
            let raw: Vec<_> = (0..n * width).map(|i| (i % 151) as f32 / 151.).collect();
            let track = FrozenTrack::from_interleaved(48000, width, 0, &raw).unwrap();
            let previews: Vec<_> = (0..5)
                .map(|_| {
                    Preview::with_channels(
                        &[PreviewLane {
                            track: &track,
                            group: Some(&group),
                            selected: None,
                            route: ChannelRoute::Merged,
                        }],
                        0..n as u64,
                        Some(model.clone()),
                        false,
                        Fades::NONE,
                    )
                    .unwrap()
                })
                .collect();
            let cpu = cpu_time();
            let wall = std::time::Instant::now();
            let mut block = [0.; 512];
            for p in &previews {
                while p.render_interleaved(&mut block, 2) > 0 {
                    std::hint::black_box(&block);
                }
            }
            println!(
                "preview channels={width} seconds=20 cpu={:.6} wall={:.6}",
                cpu_time() - cpu,
                wall.elapsed().as_secs_f64()
            );
        }
    }
}
