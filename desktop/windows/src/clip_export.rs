//! Shared export path for dragging and saving clips.

use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};

use backtracking_core::{
    ChannelRoute, Fades, FrozenTrack, TakeLabel, WavExportProfile,
    export_channel_range_with_profile, export_labelled_tracks,
};

use crate::addons::AddonHandle;
use crate::controller::LaneGroup;
use crate::preferences::WideExportMode;

#[derive(Clone, Default)]
pub(crate) struct ExportOptions {
    /// Groups are captured with the frozen audio, so later format changes cannot relabel it.
    pub(crate) groups: [Option<LaneGroup>; 2],
    pub(crate) wide_mode: WideExportMode,
    pub(crate) addons: Option<AddonHandle>,
}

/// Writes a frozen clip on a worker thread. Wide lanes use the selected group;
/// mono and stereo retain the existing routing and file naming behavior.
/// `file_labels` names each track's files and `fades` ramp every file, wide lanes included.
#[allow(clippy::too_many_arguments)]
pub(crate) fn export_clip(
    output: &Path,
    tracks: &[FrozenTrack],
    frames: Range<usize>,
    fades: Fades,
    profile: WavExportProfile,
    routes: &[ChannelRoute],
    force_mono: bool,
    file_labels: &[TakeLabel],
    options: &ExportOptions,
) -> io::Result<Vec<PathBuf>> {
    if tracks.is_empty()
        || tracks.len() > 2
        || routes.len() != tracks.len()
        || file_labels.len() != tracks.len()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid capture lanes",
        ));
    }
    let mut paths = Vec::new();
    let write = (|| {
        if tracks.iter().all(|track| track.channels() <= 2) {
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
                    let group = options.groups[lane].as_ref();
                    let channels =
                        group.map_or(0..track.channels(), |group| group.channels.clone());
                    let mut labels: Vec<String> = (1..=track.channels())
                        .map(|channel| format!("Channel {channel}"))
                        .collect();
                    if let Some(group) = group {
                        if group.channels.is_empty()
                            || group.channels.end > track.channels()
                            || group.labels.len() != group.channels.len()
                        {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidInput,
                                "The selected channel group does not match this capture",
                            ));
                        }
                        labels[group.channels.clone()].clone_from_slice(&group.labels);
                    }
                    export_channel_range_with_profile(
                        output,
                        track,
                        frames.clone(),
                        channels,
                        &labels,
                        profile,
                        options.wide_mode.export_mode(),
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
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn options() -> ExportOptions {
        ExportOptions {
            groups: [
                Some(LaneGroup {
                    id: "objects".into(),
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
        let paths = export_clip(
            &folder.0,
            std::slice::from_ref(&track),
            0..2,
            Fades::NONE,
            WavExportProfile::default(),
            &[ChannelRoute::Side],
            true,
            &[TakeLabel::default()],
            &options,
        )
        .unwrap();
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
        let paths = export_clip(
            &folder.0,
            &[track],
            0..2,
            Fades::NONE,
            WavExportProfile::default(),
            &[ChannelRoute::Left],
            true,
            &[TakeLabel::default()],
            &options,
        )
        .unwrap();
        assert_eq!(paths.len(), 1);
        let data = std::fs::read(&paths[0]).unwrap();
        assert_eq!(u16::from_le_bytes(data[22..24].try_into().unwrap()), 3);
        let samples: Vec<f32> = data[data.len() - 24..]
            .chunks_exact(4)
            .map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap()))
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
            label: String::new(),
            channels: 5..8,
            labels: vec!["A".into(), "B".into(), "C".into()],
        });
        assert!(
            export_clip(
                &folder.0,
                &[track.clone(), track],
                0..2,
                Fades::NONE,
                WavExportProfile::default(),
                &[ChannelRoute::Merged; 2],
                false,
                &[TakeLabel::default(), TakeLabel::default()],
                &options
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
        let paths = export_clip(
            &folder.0,
            &[wide.clone(), stereo],
            0..4,
            Fades::NONE,
            WavExportProfile::default(),
            &[ChannelRoute::Merged; 2],
            false,
            &labels,
            &ExportOptions::default(),
        )
        .unwrap();
        let names = file_names(&paths);
        assert_eq!(names.len(), 4, "{names:?}");
        for (channel, name) in names[..3].iter().enumerate() {
            assert!(name.starts_with("Backtracking_Test tone_A_2"), "{name}");
            let channel = channel + 1;
            assert!(
                name.ends_with(&format!("s-Ch{channel:02}_Channel {channel}.wav")),
                "{name}"
            );
        }
        assert!(
            names[3].starts_with("Backtracking_System audio_B_2"),
            "{names:?}"
        );
        assert!(names[3].ends_with("s.wav"), "{names:?}");
        // Housekeeping knows every one. By name, files sort by source first, so each source's
        // files stay together, and a take's channels sort in channel order.
        assert!(
            names.iter().all(|name| is_export_file_name(name)),
            "{names:?}"
        );
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(sorted[0], names[3]);
        assert_eq!(sorted[1..], names[..3]);

        // Without a source name the lane letter follows the take, then the channel follows it.
        let paths = export_clip(
            &folder.0,
            std::slice::from_ref(&wide),
            0..4,
            Fades::NONE,
            WavExportProfile::default(),
            &[ChannelRoute::Merged],
            false,
            &[TakeLabel::new("", Some(Lane::A))],
            &ExportOptions::default(),
        )
        .unwrap();
        for name in file_names(&paths) {
            assert!(name.starts_with("Backtracking_2"), "{name}");
            assert!(name.contains("s-A-Ch0"), "{name}");
            assert!(is_export_file_name(&name), "{name}");
        }
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
            let paths = export_clip(
                &folder.0,
                std::slice::from_ref(&wide),
                0..frames as usize,
                fades,
                WavExportProfile::default(),
                &[ChannelRoute::Merged],
                false,
                &[TakeLabel::default()],
                &options,
            )
            .unwrap();
            assert_eq!(
                paths.len(),
                if mode == WideExportMode::Multichannel {
                    1
                } else {
                    3
                }
            );
            for path in &paths {
                let data = std::fs::read(path).unwrap();
                let channels = usize::from(u16::from_le_bytes(data[22..24].try_into().unwrap()));
                let samples: Vec<f32> = data[data.len() - frames as usize * channels * 4..]
                    .chunks_exact(4)
                    .map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap()))
                    .collect();
                for (frame, samples) in samples.chunks_exact(channels).enumerate() {
                    assert!(
                        samples.iter().all(|&sample| sample == gains[frame]),
                        "{mode:?}"
                    );
                }
            }
        }
    }
}
