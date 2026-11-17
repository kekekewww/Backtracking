//! Shared export path for dragging and saving clips.

use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};

use backtracking_core::{
    ChannelRoute, FrozenTrack, WavExportProfile, export_channel_range_with_profile,
    export_routed_tracks_with_profile,
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
pub(crate) fn export_clip(
    output: &Path,
    tracks: &[FrozenTrack],
    frames: Range<usize>,
    profile: WavExportProfile,
    routes: &[ChannelRoute],
    force_mono: bool,
    options: &ExportOptions,
) -> io::Result<Vec<PathBuf>> {
    if tracks.is_empty() || tracks.len() > 2 || routes.len() != tracks.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid capture lanes",
        ));
    }
    let mut paths = Vec::new();
    let write = (|| {
        if tracks.iter().all(|track| track.channels() <= 2) {
            paths = export_routed_tracks_with_profile(
                output,
                tracks,
                frames.clone(),
                profile,
                routes,
                force_mono,
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
                    )?
                } else {
                    export_routed_tracks_with_profile(
                        output,
                        std::slice::from_ref(track),
                        frames.clone(),
                        profile,
                        &routes[lane..=lane],
                        force_mono,
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
            WavExportProfile::default(),
            &[ChannelRoute::Side],
            true,
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
            WavExportProfile::default(),
            &[ChannelRoute::Left],
            true,
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
                WavExportProfile::default(),
                &[ChannelRoute::Merged; 2],
                false,
                &options
            )
            .is_err()
        );
        assert_eq!(std::fs::read_dir(&folder.0).unwrap().count(), 0);
    }
}
