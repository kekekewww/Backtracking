use std::collections::HashSet;
use std::ops::Range;

use backtracking_addon::protocol::Source;
use backtracking_core::{DEFAULT_MAX_BUFFER_SAMPLES, MeterReadings};

/// A contiguous group in the lane's captured channel order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LaneGroup {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) channels: Range<usize>,
    pub(crate) labels: Vec<String>,
}

pub(crate) fn lane_groups(
    source: Option<&Source>,
    channels: usize,
) -> Result<Vec<LaneGroup>, String> {
    combined_channels(channels, None)?;
    let Some(source) = source else {
        return Ok(vec![numbered_group("", "", channels)]);
    };
    source.validate().map_err(|error| error.to_string())?;
    let Some(groups) = &source.groups else {
        return Ok(vec![numbered_group("channels", "Channels", channels)]);
    };
    let declared: usize = groups.iter().map(|group| group.channels.len()).sum();
    if declared != channels {
        return Err(format!(
            "Add-on groups describe {declared} channels, but its Sender has {channels}"
        ));
    }
    let mut seen = HashSet::new();
    let mut offset = 0;
    groups
        .iter()
        .map(|group| {
            if !seen.insert(group.id.as_str()) {
                return Err(format!("Add-on repeats the group id '{}'", group.id));
            }
            let end = offset + group.channels.len();
            let result = LaneGroup {
                id: group.id.clone(),
                label: group.label.clone(),
                channels: offset..end,
                labels: group.channels.clone(),
            };
            offset = end;
            Ok(result)
        })
        .collect()
}

fn numbered_group(id: &str, label: &str, channels: usize) -> LaneGroup {
    LaneGroup {
        id: id.into(),
        label: label.into(),
        channels: 0..channels,
        labels: (1..=channels)
            .map(|channel| format!("Channel {channel}"))
            .collect(),
    }
}

/// Retained audio receives one eighth of RAM, between 256 MiB and 2 GiB.
/// This budget counts interleaved f32 samples, not caches or frozen history.
pub(crate) fn history_budget_samples(physical_bytes: u64) -> usize {
    const MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
    let minimum = DEFAULT_MAX_BUFFER_SAMPLES as u64 * size_of::<f32>() as u64;
    ((physical_bytes / 8).clamp(minimum, MAX_BYTES) / size_of::<f32>() as u64) as usize
}

pub(crate) fn machine_history_budget_samples() -> usize {
    #[repr(C)]
    struct MemoryStatus {
        length: u32,
        memory_load: u32,
        total_physical: u64,
        available_physical: u64,
        total_page_file: u64,
        available_page_file: u64,
        total_virtual: u64,
        available_virtual: u64,
        available_extended_virtual: u64,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GlobalMemoryStatusEx(status: *mut MemoryStatus) -> i32;
    }
    let mut status = MemoryStatus {
        length: size_of::<MemoryStatus>() as u32,
        memory_load: 0,
        total_physical: 0,
        available_physical: 0,
        total_page_file: 0,
        available_page_file: 0,
        total_virtual: 0,
        available_virtual: 0,
        available_extended_virtual: 0,
    };
    // The struct matches MEMORYSTATUSEX and remains alive for the complete call.
    if unsafe { GlobalMemoryStatusEx(&mut status) } != 0 {
        history_budget_samples(status.total_physical)
    } else {
        DEFAULT_MAX_BUFFER_SAMPLES
    }
}

pub(crate) fn combined_channels(a: usize, b: Option<usize>) -> Result<(), String> {
    if !(1..=64).contains(&a) || b.is_some_and(|channels| !(1..=64).contains(&channels)) {
        return Err("Each lane must contain between 1 and 64 channels".into());
    }
    if a + b.unwrap_or(0) > 64 {
        return Err(format!(
            "Lane B cannot start: lanes A and B would contain {} channels; the limit is 64",
            a + b.unwrap_or(0)
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LaneMeterReadings {
    pub(crate) peak_dbfs: f32,
    pub(crate) vu: f32,
    pub(crate) momentary_lufs: Option<f32>,
}

pub(crate) fn lane_meter_readings(readings: MeterReadings, channels: usize) -> LaneMeterReadings {
    LaneMeterReadings {
        peak_dbfs: readings.peak_dbfs,
        vu: readings.vu,
        momentary_lufs: (channels <= 2).then_some(readings.momentary_lufs),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use backtracking_addon::protocol::{ChannelGroup, Format};

    fn source(groups: Option<Vec<ChannelGroup>>) -> Source {
        Source {
            id: "main".into(),
            name: "Example".into(),
            sender: "abc-1".into(),
            formats: vec![Format {
                id: "native".into(),
                label: "Native".into(),
            }],
            format: "native".into(),
            groups,
            transport: None,
        }
    }

    fn group(id: &str, labels: &[&str]) -> ChannelGroup {
        ChannelGroup {
            id: id.into(),
            label: id.into(),
            channels: labels.iter().map(|label| (*label).into()).collect(),
            positioned: false,
        }
    }

    #[test]
    fn explicit_groups_keep_stream_ranges_and_labels() {
        let source = source(Some(vec![
            group("bed", &["L", "R", "C", "LFE", "Ls", "Rs"]),
            group("objects", &["Object 1", "Object 2"]),
        ]));
        let groups = lane_groups(Some(&source), 8).unwrap();
        assert_eq!(groups[0].channels, 0..6);
        assert_eq!(groups[1].channels, 6..8);
        assert_eq!(groups[1].labels, ["Object 1", "Object 2"]);
        assert_eq!(groups[0].id, "bed");
        assert_eq!(groups[0].label, "bed");
    }

    #[test]
    fn plain_and_omitted_groups_are_numbered_without_extra_channels() {
        let plain = lane_groups(None, 1).unwrap();
        assert_eq!(plain.len(), 1);
        assert_eq!(plain[0].id, "");
        assert_eq!(plain[0].label, "");
        assert_eq!(plain[0].channels, 0..1);
        assert_eq!(plain[0].labels, ["Channel 1"]);
        let addon = lane_groups(Some(&source(None)), 12).unwrap();
        assert_eq!(addon[0].id, "channels");
        assert_eq!(addon[0].label, "Channels");
        assert_eq!(addon[0].labels.len(), 12);
        assert_eq!(addon[0].labels[11], "Channel 12");
    }

    #[test]
    fn invalid_group_layout_is_rejected_before_capture() {
        let wrong_total = source(Some(vec![group("bed", &["L", "R"])]));
        assert!(
            lane_groups(Some(&wrong_total), 6)
                .unwrap_err()
                .contains("Sender has 6")
        );
        let repeated = source(Some(vec![group("bed", &["L"]), group("bed", &["R"])]));
        assert!(
            lane_groups(Some(&repeated), 2)
                .unwrap_err()
                .contains("repeats")
        );
        let empty = source(Some(vec![group("bed", &[])]));
        assert!(lane_groups(Some(&empty), 1).is_err());
        assert!(lane_groups(Some(&source(Some(vec![]))), 1).is_err());
        assert!(lane_groups(None, 0).is_err());
        assert!(lane_groups(None, 65).is_err());
    }

    #[test]
    fn history_budget_follows_physical_memory_and_bounds() {
        const GIB: u64 = 1024 * 1024 * 1024;
        assert_eq!(history_budget_samples(0), DEFAULT_MAX_BUFFER_SAMPLES);
        assert_eq!(history_budget_samples(GIB), DEFAULT_MAX_BUFFER_SAMPLES);
        assert_eq!(history_budget_samples(2 * GIB), DEFAULT_MAX_BUFFER_SAMPLES);
        assert_eq!(history_budget_samples(8 * GIB), (GIB / 4) as usize);
        assert_eq!(history_budget_samples(16 * GIB), (2 * GIB / 4) as usize);
        assert_eq!(history_budget_samples(u64::MAX), (2 * GIB / 4) as usize);
        assert_eq!(history_budget_samples(8 * GIB + 31), (GIB / 4) as usize);
    }

    #[test]
    fn two_lanes_allow_sixty_four_channels_and_refuse_sixty_five() {
        assert!(combined_channels(64, None).is_ok());
        assert!(combined_channels(63, Some(1)).is_ok());
        assert!(combined_channels(32, Some(32)).is_ok());
        assert!(
            combined_channels(64, Some(1))
                .unwrap_err()
                .contains("Lane B")
        );
        assert!(combined_channels(40, Some(40)).is_err());
        for count in [0, 65, usize::MAX] {
            assert!(combined_channels(count, None).is_err());
            assert!(combined_channels(1, Some(count)).is_err());
        }
    }

    #[test]
    fn wide_meter_preserves_peak_and_vu_without_lufs() {
        let meter = MeterReadings {
            peak_dbfs: -3.0,
            vu: -12.0,
            momentary_lufs: -18.0,
        };
        for channels in [1, 2] {
            assert_eq!(
                lane_meter_readings(meter, channels).momentary_lufs,
                Some(-18.0)
            );
        }
        for channels in [3, 6, 12, 64] {
            let wide = lane_meter_readings(meter, channels);
            assert_eq!(wide.peak_dbfs, -3.0);
            assert_eq!(wide.vu, -12.0);
            assert_eq!(wide.momentary_lufs, None);
        }
    }
}
