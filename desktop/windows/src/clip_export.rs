//! Shared export path for dragging and saving clips.

use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};

use backtracking_core::{
    ChannelExportMode, ChannelRoute, Fades, FrozenPositionTrack, FrozenTrack, TakeLabel,
    TrackAudio, WavExportProfile, export_channel_range_with_profile, export_labelled_tracks,
    wide_export::{WideExportGroup, WideExportResult, WideRowExport, export_wide_track},
};

use crate::addons::AddonHandle;
use crate::controller::LaneGroup;
use crate::preferences::WideExportMode;

#[derive(Clone, Default)]
pub(crate) struct ExportOptions {
    /// Groups are captured with the frozen audio, so later format changes cannot relabel it.
    /// One per track, in physical snapshot order; a track past the end has none.
    pub(crate) groups: Vec<Option<LaneGroup>>,
    pub(crate) wide_mode: WideExportMode,
    pub(crate) addons: Option<AddonHandle>,
}

/// What one lane of a clip writes. A wide lane writes its channels in the export mode, and the
/// L/R pair when `pair` is set; a stereo lane with one channel chosen writes that side as mono.
#[derive(Default)]
pub(crate) struct RowExport<'a> {
    /// None selects every native channel of the displayed group. An empty list selects only the pair.
    pub(crate) channels: Option<Vec<usize>>,
    pub(crate) pair: bool,
    pub(crate) pair_binaural: bool,
    pub(crate) bed: Option<&'a LaneGroup>,
    pub(crate) positions: Option<&'a FrozenPositionTrack>,
}

pub(crate) struct ExportResult {
    pub(crate) paths: Vec<PathBuf>,
    pub(crate) objects_in_mono: bool,
}

/// Writes a frozen clip on a worker thread: of each of the 1 to `MAX_LANES` tracks, the rows in
/// `rows`, in physical snapshot order. Wide lanes use their group in `options`; mono and stereo keep the existing
/// routing and file naming. `file_labels` names each track's files and `fades` ramp every file,
/// wide lanes included.
#[allow(clippy::too_many_arguments)]
pub(crate) fn export_rows(
    output: &Path,
    tracks: &[FrozenTrack],
    frames: Range<usize>,
    fades: Fades,
    profile: WavExportProfile,
    routes: &[ChannelRoute],
    force_mono: bool,
    file_labels: &[TakeLabel],
    options: &ExportOptions,
    rows: &[RowExport<'_>],
) -> io::Result<ExportResult> {
    if tracks.is_empty()
        || tracks.len() > crate::controller::MAX_LANES
        || rows.len() < tracks.len()
        || routes.len() != tracks.len()
        || file_labels.len() != tracks.len()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid capture lanes",
        ));
    }
    // The channels of a lane only some of whose rows were selected.
    let mut narrowed = vec![None; tracks.len()];
    for (lane, track) in tracks.iter().enumerate() {
        let Some(indices) = &rows[lane].channels else {
            continue;
        };
        if indices
            .iter()
            .enumerate()
            .any(|(i, c)| *c >= track.channels() || indices[..i].contains(c))
            || (indices.is_empty() && (track.channels() <= 2 || !rows[lane].pair))
        {
            return Err(invalid_selection());
        }
        if track.channels() <= 2 && indices.len() == 1 && track.channels() == 2 {
            narrowed[lane] = Some(indices[0]..indices[0] + 1);
        }
    }
    let mut objects_in_mono = false;
    let mut paths = Vec::new();
    let write: io::Result<()> = (|| {
        if tracks.iter().all(|track| track.channels() <= 2) && narrowed.iter().all(Option::is_none)
        {
            paths = export_labelled_tracks(
                output,
                tracks,
                frames.clone(),
                profile,
                routes,
                force_mono,
                // System audio has no tempo to write into the file.
                None,
                fades,
                file_labels,
            )?;
        } else {
            for (lane, track) in tracks.iter().enumerate() {
                let files = if track.channels() > 2 {
                    let wide = write_wide(
                        output,
                        track,
                        frames.clone(),
                        fades,
                        profile,
                        &file_labels[lane],
                        options.groups.get(lane).and_then(Option::as_ref),
                        options.wide_mode,
                        &rows[lane],
                    )?;
                    objects_in_mono |= wide.objects_in_mono;
                    wide.paths
                } else if let Some(channels) = narrowed[lane].clone() {
                    // One side of a stereo lane: one mono file, named like the lane's own.
                    export_channel_range_with_profile(
                        output,
                        track,
                        frames.clone(),
                        channels,
                        &[],
                        profile,
                        ChannelExportMode::Multichannel,
                        fades,
                        &file_labels[lane],
                    )?
                } else {
                    export_labelled_tracks(
                        output,
                        std::slice::from_ref(track),
                        frames.clone(),
                        profile,
                        &routes[lane..=lane],
                        force_mono,
                        None,
                        fades,
                        &file_labels[lane..=lane],
                    )?
                };
                paths.extend(files);
            }
        }
        // Add-on notifications and native file drags always use absolute paths.
        for path in &mut paths {
            *path = dunce::canonicalize(&*path)?;
        }
        Ok(())
    })();
    if let Err(error) = write {
        for path in paths {
            let _ = std::fs::remove_file(path);
        }
        return Err(error);
    }
    if let Some(addons) = &options.addons {
        let source_frames = frames.end.saturating_sub(frames.start) as u128;
        let rate = u128::from(tracks[0].sample_rate());
        let output_frames = ((source_frames * u128::from(profile.sample_rate) + rate / 2) / rate)
            .max(u128::from(source_frames != 0)) as u64;
        let _ = addons.clip_exported(paths.clone(), profile.sample_rate, output_frames);
    }
    Ok(ExportResult {
        paths,
        objects_in_mono,
    })
}

fn invalid_selection() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "The selected rows do not match this capture",
    )
}

#[allow(clippy::too_many_arguments)]
fn write_wide(
    output: &Path,
    track: &impl TrackAudio,
    frames: Range<usize>,
    fades: Fades,
    profile: WavExportProfile,
    label: &TakeLabel,
    shown: Option<&LaneGroup>,
    mode: WideExportMode,
    rows: &RowExport<'_>,
) -> io::Result<WideExportResult> {
    fn export_group(group: &LaneGroup) -> WideExportGroup<'_> {
        WideExportGroup {
            channels: group.channels.clone(),
            labels: &group.labels,
            positioned: group.positioned,
        }
    }

    let shown = shown.map(export_group);
    let bed = rows.bed.map(export_group);
    export_wide_track(
        output,
        track,
        frames,
        fades,
        profile,
        label,
        shown.as_ref(),
        mode.into(),
        &WideRowExport {
            channels: rows.channels.as_deref(),
            pair: rows.pair,
            pair_binaural: rows.pair_binaural,
            bed: bed.as_ref(),
            positions: rows.positions,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use backtracking_core::spatial;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Folder(PathBuf);

    impl Folder {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let name = format!("backtracking-wide-export-{}-{nonce}", std::process::id());
            Self(std::env::temp_dir().join(name))
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    // A virtual frozen lane repeats the same 500 Hz tone. It makes the exporter's
    // memory visible separately from the recorder's ten minutes of retained audio.
    struct Tone {
        width: usize,
        frames: usize,
        cycle: Vec<f32>,
    }
    impl Tone {
        fn new(width: usize, frames: usize) -> Self {
            let cycle = (0..96)
                .flat_map(|i| {
                    std::iter::repeat_n(
                        (std::f64::consts::TAU * (i as f64 + 0.25) / 96.).sin() as f32 * 0.02,
                        width,
                    )
                })
                .collect();
            Self {
                width,
                frames,
                cycle,
            }
        }
    }
    impl TrackAudio for Tone {
        fn channels(&self) -> usize {
            self.width
        }
        fn sample_rate(&self) -> u32 {
            48000
        }
        fn frame_range(&self) -> Range<u64> {
            0..self.frames as u64
        }
        fn read_frames(&self, first: usize, mut output: &mut [f32]) -> bool {
            if first + output.len() / self.width > self.frames {
                return false;
            }
            let mut start = (first % 96) * self.width;
            while !output.is_empty() {
                let count = output.len().min(self.cycle.len() - start);
                output[..count].copy_from_slice(&self.cycle[start..start + count]);
                output = &mut output[count..];
                start = 0;
            }
            true
        }
    }

    fn moving_positions(channels: Range<usize>, frames: usize) -> FrozenPositionTrack {
        let mut positions = backtracking_core::PositionTrack::new();
        for c in channels {
            for frame in (0..=frames + 960).step_by(960) {
                let angle = std::f64::consts::TAU * frame as f64 / 192000.;
                positions.insert(
                    c,
                    backtracking_core::PositionKey {
                        frame: frame as u64,
                        position: backtracking_core::Position {
                            x: angle.sin(),
                            y: angle.cos(),
                            z: 0.,
                            size: 0.,
                            gain: 1.,
                        },
                    },
                );
            }
        }
        positions.freeze()
    }

    fn wav_window(path: &Path, offset: usize, count: usize) -> (usize, Vec<f32>) {
        use std::io::{Read, Seek, SeekFrom};
        let mut file = std::fs::File::open(path).unwrap();
        file.seek(SeekFrom::Start(22)).unwrap();
        let mut width = [0; 2];
        file.read_exact(&mut width).unwrap();
        let width = u16::from_le_bytes(width) as usize;
        file.seek(SeekFrom::Start(12)).unwrap();
        loop {
            let mut header = [0; 8];
            file.read_exact(&mut header).unwrap();
            let bytes = u32::from_le_bytes(header[4..].try_into().unwrap());
            if &header[..4] == b"data" {
                assert!(offset + count <= bytes as usize / (4 * width));
                break;
            }
            file.seek(SeekFrom::Current(i64::from(bytes + bytes % 2)))
                .unwrap();
        }
        file.seek(SeekFrom::Current((offset * width * 4) as i64))
            .unwrap();
        let mut data = vec![0; count * width * 4];
        file.read_exact(&mut data).unwrap();
        (
            width,
            data.as_chunks::<4>()
                .0
                .iter()
                .map(|&bytes| f32::from_le_bytes(bytes))
                .collect(),
        )
    }

    #[test]
    #[ignore = "run each release workload in its own process to measure peak working set"]
    fn export_benchmark() {
        let seconds = std::env::var("W3B_SECONDS")
            .unwrap()
            .parse::<usize>()
            .unwrap();
        let moving = std::env::var("W3B_MOVING").as_deref() == Ok("1");
        let mode = WideExportMode::ALL
            .into_iter()
            .find(|m| m.preference() == std::env::var("W3B_MODE").unwrap())
            .unwrap();
        let folder = Folder::new();
        let width = std::env::var("W3B_WIDTH")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(if moving { 16 } else { 12 });
        let tone = Tone::new(width, seconds * 48000);
        let names = [
            "FL", "FR", "FC", "LFE", "Ls", "Rs", "Lb", "Rb", "Ltf", "Rtf", "Ltr", "Rtr",
        ];
        let group = LaneGroup {
            id: if moving { "objects" } else { "channels" }.into(),
            positioned: moving,
            label: String::new(),
            channels: 0..width,
            labels: (0..width)
                .map(|i| {
                    if moving {
                        format!("Object {}", i + 1)
                    } else {
                        names[i % names.len()].into()
                    }
                })
                .collect(),
        };
        let positions = moving.then(|| moving_positions(0..width, tone.frames));
        let start = std::time::Instant::now();
        let result = write_wide(
            &folder.0,
            &tone,
            0..tone.frames,
            Fades::NONE,
            WavExportProfile::default(),
            &TakeLabel::default(),
            Some(&group),
            mode,
            &RowExport {
                pair: !moving,
                positions: positions.as_ref(),
                ..Default::default()
            },
        )
        .unwrap();
        let elapsed = start.elapsed();
        let mut worst = 0f64;
        let mut zeros = 0;
        let mut crossings = (usize::MAX, 0usize);
        for path in result
            .paths
            .iter()
            .filter(|p| p.extension().unwrap() == "wav")
        {
            for first in [4096, tone.frames / 2, tone.frames - 960] {
                let (channels, samples) = wav_window(path, first, 960);
                for side in 0..channels {
                    let mut sq = 0.;
                    let mut peak = 0f64;
                    let mut last = None;
                    let mut previous = samples[side];
                    for (i, &x) in samples[side..].iter().step_by(channels).enumerate() {
                        assert!(x.is_finite());
                        zeros += usize::from(x == 0.);
                        peak = peak.max(x.abs() as f64);
                        sq += f64::from(x).powi(2);
                        if x.is_sign_negative() != previous.is_sign_negative() {
                            if let Some(old) = last {
                                crossings = (crossings.0.min(i - old), crossings.1.max(i - old));
                            }
                            last = Some(i);
                        }
                        previous = x;
                    }
                    worst = worst
                        .max(((sq / 960.).sqrt() / peak - std::f64::consts::FRAC_1_SQRT_2).abs());
                }
            }
        }
        println!(
            "BENCH {seconds} {width} {mode:?} seconds={:.6} files={} max_rms_peak_error={worst:.8} zeros={zeros} crossing_spacing={crossings:?}",
            elapsed.as_secs_f64(),
            result.paths.len()
        );
        if !moving {
            assert!(worst < 0.001 && zeros == 0 && crossings == (48, 48));
        }
    }

    #[test]
    fn positions_match_memory_preview_and_sum_to_the_mix() {
        use backtracking_core::{
            Preview,
            waveform::{PairGroup, PreviewLane},
        };
        let folder = Folder::new();
        let tone = Tone::new(4, 192100);
        let mut samples = vec![0.; 4 * tone.frames];
        assert!(tone.read_frames(0, &mut samples));
        let track = FrozenTrack::from_interleaved(48000, 4, 0, &samples).unwrap();
        let positions = moving_positions(1..3, tone.frames);
        let group = LaneGroup {
            id: "objects".into(),
            positioned: true,
            label: "Objects".into(),
            channels: 1..4,
            labels: vec!["Moving 1".into(), "Moving 2".into(), "Unknown".into()],
        };
        let frames = 503..191773;
        let fades = Fades::new(93, 109);
        let row = RowExport {
            positions: Some(&positions),
            ..Default::default()
        };
        let export = |mode| {
            write_wide(
                &folder.0,
                &track,
                frames.clone(),
                fades,
                WavExportProfile::default(),
                &TakeLabel::default(),
                Some(&group),
                mode,
                &row,
            )
            .unwrap()
        };
        let files = export(WideExportMode::BinauralFiles);
        assert!(files.objects_in_mono);
        let names = file_names(&files.paths);
        assert!(names[0].ends_with("-Ch02_Moving 1_binaural.wav"));
        assert!(names[2].ends_with("-Ch04_Unknown.wav"));
        assert!(
            names
                .iter()
                .all(|n| backtracking_core::is_export_file_name(n))
        );
        let mix = export(WideExportMode::BinauralMix);
        assert!(mix.objects_in_mono);
        assert!(file_names(&mix.paths)[1].ends_with("-Mix-Binaural.wav"));
        let json: serde_json::Value =
            serde_json::from_reader(std::fs::File::open(files.paths.last().unwrap()).unwrap())
                .unwrap();
        assert_eq!(json["frames"], frames.len());
        assert_eq!(json["sample_rate"], 48000);
        let channels = json["channels"].as_array().unwrap();
        assert_eq!(channels.len(), 2);
        for (i, channel) in channels.iter().enumerate() {
            assert_eq!(channel["file"], names[i]);
            assert_eq!(channel["channel"], i + 2);
            let keys = channel["keys"].as_array().unwrap();
            assert_eq!(keys[0]["frame"], 0);
            assert_eq!(keys.last().unwrap()["frame"], frames.len() - 1);
            for key in keys {
                let frame = frames.start + key["frame"].as_u64().unwrap() as usize;
                let angle = std::f64::consts::TAU * frame as f64 / 192000.;
                assert!((key["x"].as_f64().unwrap() - angle.sin()).abs() < 0.0002);
                assert!((key["y"].as_f64().unwrap() - angle.cos()).abs() < 0.0002);
            }
        }
        let (width, actual) = wav_window(&mix.paths[1], 0, frames.len());
        assert_eq!(width, 2);
        let group = PairGroup::new(1, &group.labels).unwrap();
        let lanes = [PreviewLane {
            track: &track,
            route: ChannelRoute::Merged,
            group: Some(&group),
            selected: Some(&[1, 2]),
        }];
        let preview = Preview::with_positions(
            &lanes,
            frames.start as u64..frames.end as u64,
            None,
            false,
            fades,
            &[Some(&positions), None],
        )
        .unwrap();
        let mut rendered = vec![0.; actual.len()];
        assert_eq!(preview.render_interleaved(&mut rendered, 2), frames.len());
        assert_eq!(rendered, actual);
        let (_, a) = wav_window(&files.paths[0], 0, frames.len());
        let (_, b) = wav_window(&files.paths[1], 0, frames.len());
        assert!(
            a.iter()
                .zip(&b)
                .zip(&actual)
                .all(|((&a, &b), &mix)| (mix - a - b).abs() < 1e-6)
        );
        for (frame, ear) in [(48000, 1), (144000, 0)] {
            let mut power = [0.; 2];
            for pair in a[(frame - frames.start) * 2..][..1920].as_chunks::<2>().0 {
                for i in 0..2 {
                    power[i] += pair[i] * pair[i];
                }
            }
            assert!(power[ear] > power[1 - ear]);
        }
        for path in files.paths.iter().take(3).chain(mix.paths.iter().take(2)) {
            for offset in [0, frames.len() - 1] {
                assert!(wav_window(path, offset, 1).1.iter().all(|&x| x == 0.));
            }
        }
    }

    #[test]
    fn wide_modes_write_selected_rows_and_the_complete_bed_pair() {
        let folder = Folder::new();
        let bed = LaneGroup {
            id: "channels".into(),
            positioned: false,
            label: "Objects".into(),
            channels: 0..3,
            labels: vec!["FL".into(), "FC".into(), "FR".into()],
        };
        let objects = LaneGroup {
            id: "sources".into(),
            positioned: true,
            label: "Sources".into(),
            channels: 3..5,
            labels: vec!["Object A".into(), "Object B".into()],
        };
        let raw: Vec<_> = (0..110).flat_map(|_| [0.2, 0.3, -0.4, 0.5, -0.6]).collect();
        let wide = FrozenTrack::from_interleaved(48000, 5, 0, &raw).unwrap();
        let stereo =
            FrozenTrack::from_interleaved(48000, 2, 0, &[0.25, 0.125].repeat(110)).unwrap();
        let weights = spatial::stereo_weights(&["FL", "FC", "FR"]);
        let mut expected = [0.; 2];
        spatial::fold_down(&raw[..3], &weights, &mut expected).unwrap();
        let positions = moving_positions(3..4, 110);
        for shown in [&bed, &objects] {
            for mode in WideExportMode::ALL {
                for (pair, indices) in [
                    (true, None),
                    (false, Some(vec![shown.channels.start])),
                    (true, Some(vec![])),
                ] {
                    let options = ExportOptions {
                        wide_mode: mode,
                        groups: vec![Some(shown.clone()), None],
                        addons: None,
                    };
                    let selected = indices.as_ref().map_or(shown.channels.len(), Vec::len);
                    let has_position = shown == &objects && selected > 0;
                    let fallback = has_position
                        && selected == 2
                        && matches!(
                            mode,
                            WideExportMode::BinauralMix | WideExportMode::BinauralFiles
                        );
                    let rows = [
                        RowExport {
                            channels: indices,
                            pair,
                            positions: Some(&positions),
                            bed: Some(&bed),
                            pair_binaural: false,
                        },
                        RowExport::default(),
                    ];
                    let result = export_rows(
                        &folder.0,
                        &[wide.clone(), stereo.clone()],
                        5..105,
                        Fades::NONE,
                        WavExportProfile::default(),
                        &[ChannelRoute::Merged; 2],
                        false,
                        &[TakeLabel::new("A", None), TakeLabel::new("B", None)],
                        &options,
                        &rows,
                    )
                    .unwrap();
                    assert_eq!(result.objects_in_mono, fallback);
                    if has_position && mode == WideExportMode::BinauralMix {
                        assert!(result.paths.iter().any(|path| {
                            path.file_name()
                                .unwrap()
                                .to_str()
                                .unwrap()
                                .ends_with("-Mix-Binaural.wav")
                        }));
                    }
                    let count = match mode {
                        WideExportMode::MonoFiles | WideExportMode::BinauralFiles => selected,
                        WideExportMode::Multichannel => usize::from(selected > 0),
                        WideExportMode::BinauralMix => {
                            usize::from(selected > 0) + usize::from(fallback)
                        }
                    };
                    assert_eq!(
                        result.paths.len(),
                        count + usize::from(pair) + usize::from(has_position) + 1
                    );
                    for path in result.paths {
                        let name = path.file_name().unwrap().to_str().unwrap();
                        assert!(backtracking_core::is_export_file_name(name));
                        if path.extension().unwrap() == "json" {
                            continue;
                        }
                        let (channels, samples) = wav_window(&path, 0, 100);
                        assert!(wav_data_frames(&path, channels) == 100);
                        if name.ends_with("-Stereo.wav") {
                            assert_eq!(samples, expected.repeat(100));
                        } else if name.starts_with("Backtracking_B_") {
                            assert_eq!(samples, [0.25, 0.125].repeat(100));
                        } else if name.ends_with("_binaural.wav") || name.ends_with("-Binaural.wav")
                        {
                            assert_eq!(channels, 2);
                        } else if name.contains("-Ch") {
                            assert_eq!(channels, 1);
                        } else {
                            assert_eq!(channels, selected);
                        }
                    }
                }
            }
        }
    }

    fn wav_data_frames(path: &Path, channels: usize) -> usize {
        let bytes = std::fs::read(path).unwrap();
        let offset = bytes.windows(4).position(|b| b == b"data").unwrap();
        u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize
            / (channels * 4)
    }

    fn options() -> ExportOptions {
        ExportOptions {
            wide_mode: WideExportMode::MonoFiles,
            groups: vec![
                Some(LaneGroup {
                    id: "objects".into(),
                    positioned: true,
                    label: "Objects".into(),
                    channels: 3..6,
                    labels: vec!["Object 1".into(), "Object 2".into(), "Object 3".into()],
                }),
                None,
            ],
            ..ExportOptions::default()
        }
    }

    #[test]
    fn wide_exports_only_the_shown_group_with_labels_or_one_multichannel_file() {
        let folder = Folder::new();
        let track = FrozenTrack::from_interleaved(
            48_000,
            6,
            100,
            &[0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7],
        )
        .unwrap();
        let mut options = options();
        let paths = export_rows(
            &folder.0,
            std::slice::from_ref(&track),
            0..2,
            Fades::NONE,
            WavExportProfile::default(),
            &[ChannelRoute::Side],
            true,
            &[TakeLabel::default()],
            &options,
            &[RowExport::default(), RowExport::default()],
        )
        .unwrap()
        .paths;
        assert_eq!(paths.len(), 3);
        for (channel, path) in paths.iter().enumerate() {
            assert!(path.is_absolute());
            assert!(
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .contains(&format!("Object {}", channel + 1))
            );
            let data = std::fs::read(path).unwrap();
            assert_eq!(u16::from_le_bytes(data[22..24].try_into().unwrap()), 1);
            let first =
                f32::from_le_bytes(data[data.len() - 8..data.len() - 4].try_into().unwrap());
            assert_eq!(first, [0.4, 0.5, 0.6][channel]);
        }
        options.wide_mode = WideExportMode::Multichannel;
        let paths = export_rows(
            &folder.0,
            &[track],
            0..2,
            Fades::NONE,
            WavExportProfile::default(),
            &[ChannelRoute::Left],
            true,
            &[TakeLabel::default()],
            &options,
            &[RowExport::default(), RowExport::default()],
        )
        .unwrap()
        .paths;
        assert_eq!(paths.len(), 1);
        let data = std::fs::read(&paths[0]).unwrap();
        assert_eq!(u16::from_le_bytes(data[22..24].try_into().unwrap()), 3);
        let samples: Vec<f32> = data[data.len() - 24..]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&bytes| f32::from_le_bytes(bytes))
            .collect();
        assert_eq!(samples, [0.4, 0.5, 0.6, 0.5, 0.6, 0.7]);
    }

    #[test]
    fn an_invalid_second_group_removes_the_first_lanes_export() {
        let folder = Folder::new();
        let track = FrozenTrack::from_interleaved(48_000, 6, 0, &[0.1; 12]).unwrap();
        let mut options = options();
        options.groups[1] = Some(LaneGroup {
            id: "bad".into(),
            positioned: false,
            label: String::new(),
            channels: 5..8,
            labels: vec!["A".into(), "B".into(), "C".into()],
        });
        assert!(
            export_rows(
                &folder.0,
                &[track.clone(), track],
                0..2,
                Fades::NONE,
                WavExportProfile::default(),
                &[ChannelRoute::Merged; 2],
                false,
                &[TakeLabel::default(), TakeLabel::default()],
                &options,
                &[RowExport::default(), RowExport::default()]
            )
            .is_err()
        );
        assert_eq!(std::fs::read_dir(&folder.0).unwrap().count(), 0);
    }

    fn file_names(paths: &[PathBuf]) -> Vec<String> {
        paths
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn wide_files_start_with_the_source_and_lane_and_end_with_their_channel() {
        use backtracking_core::{Lane, is_export_file_name};

        let folder = Folder::new();
        let wide = FrozenTrack::from_interleaved(48_000, 3, 0, &[0.5; 3 * 4]).unwrap();
        let stereo = FrozenTrack::from_interleaved(48_000, 2, 0, &[0.25; 2 * 4]).unwrap();
        let labels = [
            TakeLabel::new("Test tone", Some(Lane::A)),
            TakeLabel::new("System audio", Some(Lane::B)),
        ];
        let paths = export_rows(
            &folder.0,
            &[wide.clone(), stereo],
            0..4,
            Fades::NONE,
            WavExportProfile::default(),
            &[ChannelRoute::Merged; 2],
            false,
            &labels,
            &ExportOptions::default(),
            &[
                RowExport {
                    pair: true,
                    ..RowExport::default()
                },
                RowExport::default(),
            ],
        )
        .unwrap()
        .paths;
        let names = file_names(&paths);
        assert_eq!(names.len(), 5, "{names:?}");
        assert!(names[3].ends_with("-Stereo.wav"));
        for (channel, name) in names[..3].iter().enumerate() {
            assert!(name.starts_with("Backtracking_Test tone_A_2"), "{name}");
            let channel = channel + 1;
            assert!(
                name.ends_with(&format!("s-Ch{channel:02}_Channel {channel}_binaural.wav")),
                "{name}"
            );
        }
        assert!(
            names[4].starts_with("Backtracking_System audio_B_2"),
            "{names:?}"
        );
        assert!(names[4].ends_with("s.wav"), "{names:?}");
        // Housekeeping knows every one. By name, files sort by source first, so each source's
        // files stay together, and a take's channels sort in channel order.
        assert!(
            names.iter().all(|name| is_export_file_name(name)),
            "{names:?}"
        );
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(sorted[0], names[4]);
        assert_eq!(sorted[1..], names[..4]);

        // Without a source name the lane letter follows the take, then the channel follows it.
        let paths = export_rows(
            &folder.0,
            std::slice::from_ref(&wide),
            0..4,
            Fades::NONE,
            WavExportProfile::default(),
            &[ChannelRoute::Merged],
            false,
            &[TakeLabel::new("", Some(Lane::A))],
            &ExportOptions::default(),
            &[
                RowExport {
                    pair: true,
                    ..RowExport::default()
                },
                RowExport::default(),
            ],
        )
        .unwrap()
        .paths;
        for name in file_names(&paths) {
            assert!(name.starts_with("Backtracking_2"), "{name}");
            assert!(
                name.contains("s-A-Ch0") || name.ends_with("s-A-Stereo.wav"),
                "{name}"
            );
            assert!(is_export_file_name(&name), "{name}");
        }
    }

    #[test]
    fn selected_rows_write_only_their_channels_and_one_side_of_stereo_as_mono() {
        let folder = Folder::new();
        // Channel n holds n / 10 in every frame.
        let wide = FrozenTrack::from_interleaved(
            48_000,
            6,
            0,
            &(0..4)
                .flat_map(|_| (0..6).map(|channel| channel as f32 / 10.0))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let stereo = FrozenTrack::from_interleaved(48_000, 2, 0, &[0.25, -0.5].repeat(4)).unwrap();
        let labels = [
            TakeLabel::new("Tone", Some(backtracking_core::Lane::A)),
            TakeLabel::new("Music", Some(backtracking_core::Lane::B)),
        ];
        let options = options();
        // Channels 2 and 3 of the wide lane, which straddle the start of its "objects" group,
        // and the right side of the stereo lane.
        let paths = export_rows(
            &folder.0,
            &[wide.clone(), stereo.clone()],
            0..4,
            Fades::NONE,
            WavExportProfile::default(),
            &[ChannelRoute::Merged; 2],
            false,
            &labels,
            &options,
            &[
                RowExport {
                    channels: Some(vec![2, 3]),
                    ..RowExport::default()
                },
                RowExport {
                    channels: Some(vec![1]),
                    ..RowExport::default()
                },
            ],
        )
        .unwrap()
        .paths;
        let names = file_names(&paths);
        assert_eq!(names.len(), 3, "{names:?}");
        assert!(names[0].ends_with("-Ch03_Channel 3.wav"), "{names:?}");
        assert!(names[1].ends_with("-Ch04_Object 1.wav"), "{names:?}");
        assert!(names[2].starts_with("Backtracking_Music_B_2"), "{names:?}");
        assert!(
            names[2].ends_with("s.wav") && !names[2].contains("-Ch"),
            "{names:?}"
        );
        for (path, value) in paths.iter().zip([0.2, 0.3, -0.5]) {
            let data = std::fs::read(path).unwrap();
            assert_eq!(u16::from_le_bytes(data[22..24].try_into().unwrap()), 1);
            let last = f32::from_le_bytes(data[data.len() - 4..].try_into().unwrap());
            assert_eq!(last, value);
        }
        // Both sides, or every channel, are the whole lane as before.
        let whole = export_rows(
            &folder.0,
            &[wide.clone(), stereo],
            0..4,
            Fades::NONE,
            WavExportProfile::default(),
            &[ChannelRoute::Merged; 2],
            false,
            &labels,
            &options,
            &[RowExport::default(), RowExport::default()],
        )
        .unwrap()
        .paths;
        assert_eq!(whole.len(), 4, "the objects group and one stereo file");
        // Channels the track does not have are refused, and nothing is left behind.
        let before = std::fs::read_dir(&folder.0).unwrap().count();
        assert!(
            export_rows(
                &folder.0,
                std::slice::from_ref(&wide),
                0..4,
                Fades::NONE,
                WavExportProfile::default(),
                &[ChannelRoute::Merged],
                false,
                &[TakeLabel::default()],
                &options,
                &[
                    RowExport {
                        channels: Some(vec![5, 6]),
                        ..RowExport::default()
                    },
                    RowExport::default(),
                ],
            )
            .is_err()
        );
        assert_eq!(std::fs::read_dir(&folder.0).unwrap().count(), before);
    }

    #[test]
    fn fades_ramp_every_channel_of_a_wide_lane_in_either_wide_mode() {
        let folder = Folder::new();
        let frames = 10;
        let wide = FrozenTrack::from_interleaved(48_000, 3, 0, &[1.0; 3 * 10]).unwrap();
        let fades = Fades::new(4, 3);
        let gains: Vec<f32> = (0..frames).map(|index| fades.gain(index, frames)).collect();
        assert!(gains[0] == 0.0 && gains[5] == 1.0 && gains[9] == 0.0);
        let mut options = ExportOptions::default();
        for mode in [WideExportMode::MonoFiles, WideExportMode::Multichannel] {
            options.wide_mode = mode;
            let paths = export_rows(
                &folder.0,
                std::slice::from_ref(&wide),
                0..frames as usize,
                fades,
                WavExportProfile::default(),
                &[ChannelRoute::Merged],
                false,
                &[TakeLabel::default()],
                &options,
                &[
                    RowExport {
                        pair: true,
                        ..RowExport::default()
                    },
                    RowExport::default(),
                ],
            )
            .unwrap()
            .paths;
            assert_eq!(
                paths.len(),
                if mode == WideExportMode::Multichannel {
                    2
                } else {
                    4
                }
            );
            for path in &paths {
                let data = std::fs::read(path).unwrap();
                let channels = usize::from(u16::from_le_bytes(data[22..24].try_into().unwrap()));
                let samples: Vec<f32> = data[data.len() - frames as usize * channels * 4..]
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|&bytes| f32::from_le_bytes(bytes))
                    .collect();
                for (frame, samples) in samples.chunks_exact(channels).enumerate() {
                    assert!(
                        samples
                            .iter()
                            .all(|&sample| (sample - gains[frame]).abs() < 1e-6),
                        "{mode:?}"
                    );
                }
            }
        }
    }
}
