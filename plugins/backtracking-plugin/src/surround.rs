use std::io::{self, Read, Seek, SeekFrom, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};

use backtracking_core::{
    ChannelRoute, Fades, FrozenTrack, TrackAudio, WavExportProfile, WaveformView,
    export_routed_tracks_with_fades,
    waveform::{PairGroup, PairSide},
};

use crate::session::WaveformPeaks;

pub(super) fn speaker_labels(bits: &[u32]) -> Vec<String> {
    let sides = bits.contains(&9) && bits.contains(&10);
    bits.iter()
        .enumerate()
        .map(|(i, bit)| {
            match bit {
                0 => "FL",
                1 => "FR",
                2 | 19 => "FC",
                3 => "LFE",
                4 if sides => "Lb",
                5 if sides => "Rb",
                4 | 9 => "Ls",
                5 | 10 => "Rs",
                6 => "FLc",
                7 => "FRc",
                8 => "BC",
                11 => "TC",
                12 => "TFL",
                13 => "TFC",
                14 => "TFR",
                15 => "TBL",
                16 => "TBC",
                17 => "TBR",
                18 => "LFE2",
                24 => "Ltm",
                25 => "Rtm",
                28 => "BtFL",
                29 => "BtFC",
                30 => "BtFR",
                59 => "Lw",
                60 => "Rw",
                _ => return format!("Channel {}", i + 1),
            }
            .to_owned()
        })
        .collect()
}

#[derive(Clone)]
pub(crate) struct WideLayout {
    pub(crate) group: PairGroup,
    pub(crate) mask: u32,
}

impl WideLayout {
    pub(super) fn new(channels: usize, labels: &[String]) -> Option<Self> {
        if channels <= 2 {
            return None;
        }
        let unnamed;
        let names = if labels.len() == channels {
            labels
        } else {
            unnamed = (1..=channels)
                .map(|n| format!("Channel {n}"))
                .collect::<Vec<_>>();
            &unnamed
        };
        // A WAV mask implies ascending speaker-bit order. Never change stream order to fit one.
        let speakers = [
            "FL", "FR", "FC", "LFE", "Lb", "Rb", "FLc", "FRc", "BC", "Ls", "Rs", "TC", "TFL",
            "TFC", "TFR", "TBL", "TBC", "TBR",
        ];
        let mask = names
            .iter()
            .try_fold(0, |mask, name| {
                let bit = 1u32 << speakers.iter().position(|s| s == name)?;
                (bit > mask).then_some(mask | bit)
            })
            .unwrap_or(0);
        Some(Self {
            group: PairGroup::new(0, names)?,
            mask,
        })
    }

    pub(super) fn peaks(
        &self,
        view: WaveformView,
        peaks: &mut WaveformPeaks,
        mut read: impl FnMut(&PairGroup, PairSide, &mut [f32], &mut [f32]),
    ) {
        let side = if view == WaveformView::Right {
            PairSide::Right
        } else {
            PairSide::Left
        };
        read(&self.group, side, &mut peaks.minimum, &mut peaks.maximum);
        if view == WaveformView::Merged {
            // Both mode has one row per input. Show the extrema of both sides of the main pair.
            let mut right = peaks.clone();
            read(
                &self.group,
                PairSide::Right,
                &mut right.minimum,
                &mut right.maximum,
            );
            for (l, r) in peaks.minimum.iter_mut().zip(right.minimum) {
                *l = l.min(r);
            }
            for (l, r) in peaks.maximum.iter_mut().zip(right.maximum) {
                *l = l.max(r);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn export(
    output: &Path,
    tracks: &[FrozenTrack],
    range: Range<usize>,
    profile: WavExportProfile,
    force_mono: bool,
    tempo: Option<f64>,
    fades: Fades,
    mask: u32,
) -> io::Result<Vec<PathBuf>> {
    let wide = tracks.first().is_some_and(|t| t.channels() > 2);
    let routes = &[ChannelRoute::Merged; 2][..tracks.len()];
    let paths = if let [main, side] = tracks
        && wide
        && force_mono
        && side.channels() == 2
    {
        // The core writer takes one mono flag for the take. Present only the sidechain as mono.
        let lanes = [
            ExportLane {
                track: main,
                mono: false,
            },
            ExportLane {
                track: side,
                mono: true,
            },
        ];
        export_routed_tracks_with_fades(
            output, &lanes, range, profile, routes, false, tempo, fades,
        )?
    } else {
        export_routed_tracks_with_fades(
            output,
            tracks,
            range,
            profile,
            routes,
            force_mono && !wide,
            tempo,
            fades,
        )?
    };
    if wide
        && mask != 0
        && let Err(err) = write_mask(&paths[0], tracks[0].channels(), mask)
    {
        for path in &paths {
            let _ = std::fs::remove_file(path);
        }
        return Err(err);
    }
    Ok(paths)
}

struct ExportLane<'a> {
    track: &'a FrozenTrack,
    mono: bool,
}

impl TrackAudio for ExportLane<'_> {
    fn sample_rate(&self) -> u32 {
        self.track.sample_rate()
    }
    fn channels(&self) -> usize {
        if self.mono { 1 } else { self.track.channels() }
    }
    fn frame_range(&self) -> Range<u64> {
        self.track.frame_range()
    }
    fn read_frames(&self, mut first: usize, output: &mut [f32]) -> bool {
        if !self.mono {
            return TrackAudio::read_frames(self.track, first, output);
        }
        let mut scratch = [0.; 512];
        for chunk in output.chunks_mut(256) {
            let stereo = &mut scratch[..chunk.len() * 2];
            if !TrackAudio::read_frames(self.track, first, stereo) {
                return false;
            }
            for (dst, pair) in chunk.iter_mut().zip(stereo.as_chunks::<2>().0) {
                *dst = 0.5 * pair[0] + 0.5 * pair[1];
            }
            first += chunk.len();
        }
        true
    }
}

fn write_mask(path: &Path, channels: usize, mask: u32) -> io::Result<()> {
    // Core emits a 40-byte extensible fmt chunk first and leaves dwChannelMask at zero.
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;
    let mut header = [0; 40];
    file.read_exact(&mut header)?;
    if &header[8..16] != b"WAVEfmt "
        || header[16..20] != 40u32.to_le_bytes()
        || header[20..22] != 0xfffeu16.to_le_bytes()
        || header[22..24] != (channels as u16).to_le_bytes()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "expected extensible WAV",
        ));
    }
    file.seek(SeekFrom::Start(40))?;
    file.write_all(&mask.to_le_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use vst3::Steinberg::Vst::SpeakerArr::*;

    #[test]
    fn wide_wav_preserves_tones_and_marks_only_representable_layouts() {
        let labels = speaker_labels(&[0, 1, 2, 3, 4, 5, 9, 10, 12, 14, 15, 17]);
        let mask = WideLayout::new(12, &labels).unwrap().mask;
        assert_eq!(mask, 0x2d63f);
        let mut reordered = labels.clone();
        reordered.swap(4, 6);
        assert_eq!(WideLayout::new(12, &reordered).unwrap().mask, 0);
        assert_eq!(WideLayout::new(10, &[]).unwrap().mask, 0);
        let frames = 4800;
        let audio: Vec<_> = (0..frames)
            .flat_map(|i| {
                (0..12).map(move |ch| {
                    (0.25
                        * ((i as f64 + 0.125) * (ch + 1) as f64 * 100. * std::f64::consts::TAU
                            / 48000.)
                            .sin()) as f32
                })
            })
            .collect();
        let tracks = [
            FrozenTrack::from_interleaved(48000, 12, 100, &audio).unwrap(),
            FrozenTrack::from_interleaved(48000, 2, 100, &[0.5, 0.25].repeat(frames)).unwrap(),
        ];
        let dir =
            std::env::temp_dir().join(format!("s3-3-wav-{}", backtracking_core::new_sender_id()));
        let paths = export(
            &dir,
            &tracks,
            0..frames,
            WavExportProfile::default(),
            true,
            None,
            Fades::NONE,
            mask,
        )
        .unwrap();
        let main = std::fs::read(&paths[0]).unwrap();
        assert_eq!(&main[22..24], &12u16.to_le_bytes());
        assert_eq!(&main[40..44], &mask.to_le_bytes());
        let data = main.windows(4).position(|c| c == b"data").unwrap() + 8;
        let actual: Vec<_> = main[data..]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect();
        assert_eq!(actual, audio);
        for ch in 0..12 {
            let samples: Vec<_> = actual.iter().skip(ch).step_by(12).copied().collect();
            let rms =
                (samples.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / frames as f64).sqrt();
            assert!((rms - 0.25 / std::f64::consts::SQRT_2).abs() < 1e-7);
            assert!(!samples.contains(&0.));
            let period = 480. / (ch + 1) as f64;
            let crossings: Vec<_> = samples
                .windows(2)
                .enumerate()
                .filter(|(_, p)| p[0] < 0. && p[1] > 0.)
                .map(|(i, _)| i)
                .collect();
            assert!(
                crossings
                    .windows(2)
                    .all(|p| ((p[1] - p[0]) as f64 - period).abs() < 1.)
            );
        }
        let side = std::fs::read(&paths[1]).unwrap();
        assert_eq!(&side[22..24], &1u16.to_le_bytes());
        let data = side.windows(4).position(|c| c == b"data").unwrap() + 8;
        assert!(
            side[data..]
                .as_chunks::<4>()
                .0
                .iter()
                .all(|b| f32::from_le_bytes(*b) == 0.375)
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn labels_follow_vst_buffer_order() {
        for (mask, names) in [
            (k51, "FL FR FC LFE Ls Rs"),
            (k71Music, "FL FR FC LFE Lb Rb Ls Rs"),
            (k51_4, "FL FR FC LFE Ls Rs TFL TFR TBL TBR"),
            (k71_2, "FL FR FC LFE Lb Rb Ls Rs Ltm Rtm"),
            (k71_4, "FL FR FC LFE Lb Rb Ls Rs TFL TFR TBL TBR"),
            (k71_6, "FL FR FC LFE Lb Rb Ls Rs TFL TFR TBL TBR Ltm Rtm"),
            (
                k91_6,
                "FL FR FC LFE Lb Rb FLc FRc Ls Rs TFL TFR TBL TBR Ltm Rtm",
            ),
            (
                k91_6_W,
                "FL FR FC LFE Lb Rb Ls Rs TFL TFR TBL TBR Ltm Rtm Lw Rw",
            ),
            (1 | (1 << 63), "FL Channel 2"),
            (0, ""),
        ] {
            let bits: Vec<_> = (0..64).filter(|i| mask & (1 << i) != 0).collect();
            assert_eq!(speaker_labels(&bits).join(" "), names);
        }
    }
}
