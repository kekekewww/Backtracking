//! Joins position timestamps to received audio, including compressed gaps.

use std::collections::VecDeque;
use std::ops::Range;

use backtracking_addon::protocol::ChannelPosition;
use backtracking_core::{FrozenPositionTrack, Position, PositionKey, PositionTrack};

// Reserve two MiB of the lane budget for pending wire entries and clock spans.
const TRACK_BYTES: usize = 6 * 1024 * 1024;
const MAX_PENDING: usize = 256;
const MAX_MAPPINGS: usize = 8192;

#[derive(Clone, Copy)]
struct Mapping {
    time: u64,
    end_time: u64,
    first_frame: u64,
    end_frame: u64,
}

struct Pending {
    time: u64,
    channels: Vec<ChannelPosition>,
}

pub(crate) struct PositionTimeline {
    track: PositionTrack,
    mappings: VecDeque<Mapping>,
    pending: VecDeque<Pending>,
    waiting_audio: VecDeque<Mapping>,
    retained_end: u64,
    passed_time: u64,
    overflowed: bool,
    warned: bool,
}

impl Default for PositionTimeline {
    fn default() -> Self {
        Self {
            track: PositionTrack::with_memory_limit(TRACK_BYTES),
            mappings: VecDeque::new(),
            pending: VecDeque::new(),
            waiting_audio: VecDeque::new(),
            retained_end: 0,
            passed_time: 0,
            overflowed: false,
            warned: false,
        }
    }
}

impl PositionTimeline {
    pub(crate) fn receive(&mut self, time: u64, channels: Vec<ChannelPosition>) {
        if self.pending.len() == MAX_PENDING {
            self.pending.pop_front();
            self.overflowed = true;
        }
        self.pending.push_back(Pending { time, channels });
        self.resolve();
    }

    /// The packet's final frame is anchored to the retained capture's end.
    /// This re-anchors after gaps instead of extending an offset through them.
    pub(crate) fn audio_packet(
        &mut self,
        time: u64,
        frames: usize,
        source_rate: u32,
        rate: u32,
        retained: Range<u64>,
    ) {
        let duration = (frames as u128 * 1_000_000_000 / u128::from(source_rate)) as u64;
        let output_frames =
            (frames as u128 * u128::from(rate)).div_ceil(u128::from(source_rate)) as u64;
        let newly_retained = retained.end.saturating_sub(self.retained_end);
        self.retained_end = self.retained_end.max(retained.end);
        let mapping = Mapping {
            time,
            end_time: time.saturating_add(duration),
            first_frame: retained
                .end
                .saturating_sub(output_frames)
                .max(retained.start),
            end_frame: retained.end,
        };
        if newly_retained == 0 {
            if let Some(last) = self.waiting_audio.back_mut()
                && last.end_time.abs_diff(time) <= 1
            {
                last.end_time = mapping.end_time;
                last.end_frame = last.end_frame.saturating_add(output_frames);
            } else {
                if self.waiting_audio.len() == MAX_PENDING {
                    self.waiting_audio.pop_front();
                }
                self.waiting_audio.push_back(Mapping {
                    first_frame: 0,
                    end_frame: output_frames,
                    ..mapping
                });
            }
            return;
        }
        // Opening the silence gate can commit earlier quiet packets as pre-roll.
        // Only actual received audio gets a span; implicit gap padding does not.
        let mut extra = newly_retained.saturating_sub(output_frames);
        if self
            .waiting_audio
            .back()
            .is_some_and(|quiet| quiet.end_time.abs_diff(time) > 2_000_000)
        {
            self.waiting_audio.clear();
        }
        let mut end_frame = mapping.first_frame;
        let mut pre_roll = Vec::new();
        while extra > 0 {
            let Some(quiet) = self.waiting_audio.pop_back() else {
                break;
            };
            let count = extra.min(quiet.end_frame - quiet.first_frame);
            let duration = quiet.end_time - quiet.time;
            let time = quiet.end_time
                - (u128::from(duration) * u128::from(count)
                    / u128::from(quiet.end_frame - quiet.first_frame)) as u64;
            pre_roll.push(Mapping {
                time,
                end_time: quiet.end_time,
                first_frame: end_frame.saturating_sub(count),
                end_frame,
            });
            extra -= count;
            end_frame = end_frame.saturating_sub(count);
        }
        self.waiting_audio.clear();
        for quiet in pre_roll.into_iter().rev() {
            self.add_mapping(quiet);
        }
        self.passed_time = self.passed_time.max(mapping.end_time);
        if mapping.first_frame < mapping.end_frame {
            self.add_mapping(mapping);
        }
        self.resolve();
    }

    fn add_mapping(&mut self, mapping: Mapping) {
        if let Some(last) = self.mappings.back_mut()
            && last.end_time == mapping.time
            && last.end_frame == mapping.first_frame
        {
            last.end_time = mapping.end_time;
            last.end_frame = mapping.end_frame;
        } else {
            if self.mappings.len() == MAX_MAPPINGS {
                self.mappings.pop_front();
                self.overflowed = true;
            }
            self.mappings.push_back(mapping);
        }
    }

    fn resolve(&mut self) {
        let mut waiting = VecDeque::new();
        while let Some(pending) = self.pending.pop_front() {
            let mapping =
                self.mappings.iter().rev().find(|mapping| {
                    mapping.time <= pending.time && pending.time < mapping.end_time
                });
            if let Some(mapping) = mapping {
                let frame = mapping.first_frame
                    + ((u128::from(pending.time - mapping.time)
                        * u128::from(mapping.end_frame - mapping.first_frame))
                        / u128::from(mapping.end_time - mapping.time)) as u64;
                for channel in pending.channels {
                    self.track.insert(
                        channel.channel as usize,
                        PositionKey {
                            frame,
                            position: Position {
                                x: channel.x,
                                y: channel.y,
                                z: channel.z,
                                size: channel.size.unwrap_or(0.0),
                                gain: channel.gain.unwrap_or(1.0),
                            },
                        },
                    );
                }
            } else if pending.time >= self.passed_time {
                waiting.push_back(pending);
            }
            // A timestamp passed without audio is a gap, never a guessed frame.
        }
        self.pending = waiting;
    }

    pub(crate) fn retain_frames(&mut self, frames: Range<u64>) {
        self.track.retain_frames(frames.clone());
        while self
            .mappings
            .front()
            .is_some_and(|mapping| mapping.end_frame <= frames.start)
        {
            self.mappings.pop_front();
        }
    }

    pub(crate) fn stop_pending(&mut self) {
        self.pending.clear();
        self.waiting_audio.clear();
    }

    /// A label change keeps future states; removing positioned channels drops
    /// only their pending entries. Already retained history remains unchanged.
    pub(crate) fn retain_channels(&mut self, allowed: &[usize]) {
        for pending in &mut self.pending {
            pending
                .channels
                .retain(|channel| allowed.contains(&(channel.channel as usize)));
        }
        self.pending.retain(|pending| !pending.channels.is_empty());
    }

    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn freeze(&self) -> FrozenPositionTrack {
        self.track.freeze()
    }

    pub(crate) fn take_overflowed(&mut self) -> bool {
        self.overflowed |= self.track.take_overflowed();
        if self.warned || !self.overflowed {
            return false;
        }
        self.warned = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update() -> Vec<ChannelPosition> {
        vec![ChannelPosition::new(2, -0.5, 0.5, 0.0)]
    }

    #[test]
    fn future_positions_wait_and_missing_audio_positions_are_dropped() {
        let mut timeline = PositionTimeline::default();
        timeline.receive(1_050_000_000, update());
        timeline.receive(2_500_000_000, update());
        assert!(timeline.freeze().is_empty());
        timeline.audio_packet(1_000_000_000, 100, 1000, 1000, 0..100);
        assert_eq!(timeline.freeze().channel_positions(2, 0..100)[0].frame, 50);
        timeline.audio_packet(3_000_000_000, 100, 1000, 1000, 0..200);
        assert_eq!(timeline.freeze().len(), 1);
        assert!(timeline.pending.is_empty());
    }

    #[test]
    fn anchors_follow_steady_audio_and_reanchor_after_a_compressed_gap() {
        let mut timeline = PositionTimeline::default();
        for packet in 0..20_u64 {
            let time = 1_000_000_000 + packet * 10_000_000;
            timeline.audio_packet(time, 10, 1000, 1000, 0..(packet + 1) * 10);
            timeline.receive(time + 5_000_000, update());
        }
        timeline.receive(5_005_000_000, update());
        timeline.audio_packet(5_000_000_000, 10, 1000, 1000, 0..210);
        let keys = timeline.freeze().channel_positions(2, 0..210);
        assert_eq!(keys[0].frame, 5);
        assert_eq!(keys[19].frame, 195);
        assert_eq!(keys[20].frame, 205);
        assert_eq!(timeline.mappings.len(), 2);
    }

    #[test]
    fn frozen_positions_survive_ageing_clear_and_stopping_new_updates() {
        let mut timeline = PositionTimeline::default();
        timeline.audio_packet(0, 100, 1000, 1000, 0..100);
        timeline.receive(10_000_000, update());
        timeline.receive(60_000_000, update());
        let frozen = timeline.freeze();
        timeline.receive(200_000_000, update());
        timeline.stop_pending();
        timeline.retain_frames(50..100);
        assert_eq!(timeline.freeze().channel_positions(2, 50..100).len(), 2);
        timeline.clear();
        assert!(timeline.freeze().is_empty());
        assert_eq!(frozen.len(), 2);
    }

    #[test]
    fn positions_for_quiet_pre_roll_wait_for_the_gate_to_keep_their_audio() {
        use backtracking_core::{DurationChoice, SynchronizedCapture};
        let mut audio = SynchronizedCapture::new(1000, 4, None, DurationChoice::Seconds10).unwrap();
        let mut timeline = PositionTimeline::default();
        timeline.receive(80_000_000, update());
        timeline.receive(20_000_000, update());
        audio
            .push_native_interleaved(0, &[0.0; 400], false)
            .unwrap();
        timeline.audio_packet(0, 100, 1000, 1000, audio.capture().frame_range());
        assert!(timeline.freeze().is_empty());
        audio
            .push_native_interleaved(100_000_000, &[0.25; 40], false)
            .unwrap();
        timeline.audio_packet(100_000_000, 10, 1000, 1000, audio.capture().frame_range());
        let keys = timeline
            .freeze()
            .channel_positions(2, audio.capture().frame_range());
        assert_eq!(
            keys.len(),
            1,
            "position in discarded quiet audio must not be stored"
        );
        assert_eq!(
            keys[0].frame, 20,
            "80ms is 20 frames into retained 60..100ms pre-roll"
        );
    }

    #[test]
    fn real_capture_positions_reanchor_after_retained_and_compressed_gaps() {
        use backtracking_core::{DurationChoice, SynchronizedCapture};
        for keep_quiet in [false, true] {
            let mut audio =
                SynchronizedCapture::new(1000, 4, None, DurationChoice::Seconds10).unwrap();
            audio.set_retain_silence(keep_quiet);
            let mut timeline = PositionTimeline::default();
            for packet in 0..20_u64 {
                let time = 1_000_000_000 + packet * 10_000_000;
                timeline.receive(time + 5_000_000, update());
                audio
                    .push_native_interleaved(time, &[0.25; 40], false)
                    .unwrap();
                timeline.audio_packet(time, 10, 1000, 1000, audio.capture().frame_range());
            }
            timeline.receive(5_005_000_000, update());
            audio
                .push_native_interleaved(5_000_000_000, &[0.25; 40], false)
                .unwrap();
            timeline.audio_packet(5_000_000_000, 10, 1000, 1000, audio.capture().frame_range());
            let keys = timeline
                .freeze()
                .channel_positions(2, audio.capture().frame_range());
            assert_eq!(keys[0].frame, 5);
            assert_eq!(keys[19].frame, 195);
            let expected = audio.capture().frame_range().end - 5;
            assert_eq!(keys[20].frame, expected, "keep quiet={keep_quiet}");
            if keep_quiet {
                assert_eq!(expected, 4005);
            }
        }
    }
}
