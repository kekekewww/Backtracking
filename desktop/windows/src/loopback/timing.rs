struct PacketEnd {
    position: u64,
    timestamp_ns: u64,
    duration_ns: u64,
}

struct IdleRun {
    origin_ns: u64,
    frames: u64,
}

pub(super) struct PacketStamp {
    pub(super) timestamp_ns: u64,
    pub(super) discontinuity: bool,
}

/// Projects device-frame positions onto the shared QPC timeline. The device
/// position determines continuity, and QPC slowly corrects independent clocks.
pub(super) struct PacketClock {
    sample_rate: u32,
    next: Option<PacketEnd>,
    idle_end_ns: Option<u64>,
    idle_run: Option<IdleRun>,
    retain_since_ns: Option<u64>,
}

impl PacketClock {
    pub(super) fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            next: None,
            idle_end_ns: None,
            idle_run: None,
            retain_since_ns: None,
        }
    }

    pub(super) fn timestamp(
        &mut self,
        position: u64,
        frames: usize,
        measured_ns: Option<u64>,
        driver_discontinuity: bool,
        now_ns: impl FnOnce() -> u64,
    ) -> PacketStamp {
        let duration_ns = self.duration_ns(frames as u64);
        let idle_end_ns = self.idle_end_ns.take();
        self.idle_run = None;
        let fallback = || now_ns().saturating_sub(duration_ns);
        let (timestamp_ns, discontinuity) = match &self.next {
            Some(previous) => {
                let missing_frames = position.checked_sub(previous.position);
                // If a virtual endpoint supplies only zero positions, use a nearby
                // QPC observation as the continuity check until a position is available.
                let adjacent_by_time = position == 0
                    && missing_frames.is_none()
                    && measured_ns.is_none_or(|measured| {
                        measured.abs_diff(previous.timestamp_ns)
                            <= (previous.duration_ns / 4).max(2_000_000)
                    });
                if !driver_discontinuity
                    && idle_end_ns.is_none()
                    && (missing_frames == Some(0) || adjacent_by_time)
                {
                    // Correct at most 1,000 ppm per packet. Bursty QPC observations
                    // must not turn into missing or duplicated device frames.
                    let expected = previous.timestamp_ns;
                    let measured = measured_ns.unwrap_or(expected);
                    let adjustment = (measured.abs_diff(expected) / 32)
                        .min(previous.duration_ns / 1_000)
                        .min(self.duration_ns(1) / 2);
                    let start = if measured < expected {
                        expected.saturating_sub(adjustment)
                    } else {
                        expected.saturating_add(adjustment)
                    };
                    (start, false)
                } else {
                    let minimum = previous.timestamp_ns.saturating_add(
                        if !driver_discontinuity && idle_end_ns.is_none() {
                            self.duration_ns(missing_frames.unwrap_or_default())
                        } else {
                            0
                        },
                    );
                    let measured = measured_ns.unwrap_or_else(|| {
                        if driver_discontinuity || idle_end_ns.is_some() || missing_frames.is_none()
                        {
                            fallback()
                        } else {
                            minimum
                        }
                    });
                    (measured.max(minimum), idle_end_ns.is_none()
                        || (driver_discontinuity && self.retain_since_ns.is_some()))
                }
            }
            None => (measured_ns.unwrap_or_else(fallback), false),
        };
        let timestamp_ns = timestamp_ns.max(idle_end_ns.unwrap_or_default());

        self.next = Some(PacketEnd {
            position: position.saturating_add(frames as u64),
            timestamp_ns: timestamp_ns.saturating_add(duration_ns),
            duration_ns,
        });
        PacketStamp {
            timestamp_ns,
            discontinuity,
        }
    }

    pub(super) fn idle_stamp(
        &mut self,
        frames: usize,
        now_ns: impl FnOnce() -> u64,
    ) -> PacketStamp {
        let duration_ns = self.duration_ns(frames as u64);
        let start = self.idle_end_ns.or_else(|| self.next.as_ref().map(|next| next.timestamp_ns))
            .unwrap_or_else(|| now_ns().saturating_sub(duration_ns));
        let discontinuity = self.next.is_some() && self.idle_run.is_none();
        self.append_idle_stamp(start, frames, discontinuity)
    }

    pub(super) fn set_retain_silence(&mut self, enabled: bool, now_ns: u64) {
        if self.retain_since_ns.is_some() != enabled {
            self.retain_since_ns = enabled.then_some(now_ns);
            self.idle_run = None;
        }
    }

    /// `until_ns` must leave room for the device's largest in-flight packet.
    pub(super) fn retained_idle_stamp(
        &mut self,
        until_ns: u64,
        max_frames: usize,
    ) -> Option<(PacketStamp, usize)> {
        let since_ns = self.retain_since_ns?;
        if max_frames == 0 {
            return None;
        }
        let mut start = self.idle_end_ns.or_else(|| self.next.as_ref().map(|next| next.timestamp_ns))
            .unwrap_or(since_ns).max(since_ns);
        let (origin_ns, mut previous_frames) = self.idle_run.as_ref()
            .filter(|run| run.origin_ns.saturating_add(self.duration_ns(run.frames)) == start)
            .map_or((start, 0), |run| (run.origin_ns, run.frames));
        let target_frames = (u128::from(until_ns.saturating_sub(origin_ns))
            * u128::from(self.sample_rate) / 1_000_000_000)
            .min(u128::from(u64::MAX)) as u64;
        if self.idle_run.is_some()
            && target_frames.saturating_sub(previous_frames)
                > u64::from(self.sample_rate).max(max_frames as u64)
        {
            // The first small packet establishes an anchor in the consumer.
            // After a long scheduler stall, jump to the latest small packet;
            // SynchronizedCapture fills that known-quiet gap in bounded work.
            previous_frames = target_frames.saturating_sub(max_frames as u64);
            start = origin_ns.saturating_add(self.duration_ns(previous_frames));
            self.idle_end_ns = Some(start);
            self.idle_run = Some(IdleRun { origin_ns, frames: previous_frames });
        }
        let frames = target_frames.saturating_sub(previous_frames)
            .min(max_frames as u64) as usize;
        if frames == 0 {
            return None;
        }
        // Keeping silence is an unfiltered continuation, not a fabricated
        // discontinuity that would fade the preceding original samples.
        Some((self.append_idle_stamp(start, frames, false), frames))
    }

    pub(super) fn idle_cutoff_ns(&self, now_ns: u64, buffer_frames: usize, guard_ns: u64) -> u64 {
        now_ns.saturating_sub(self.duration_ns(buffer_frames as u64)).saturating_sub(guard_ns)
    }

    fn append_idle_stamp(&mut self, start: u64, frames: usize, discontinuity: bool) -> PacketStamp {
        let (origin_ns, previous_frames) = self.idle_run.as_ref()
            .filter(|run| run.origin_ns.saturating_add(self.duration_ns(run.frames)) == start)
            .map_or((start, 0), |run| (run.origin_ns, run.frames));
        let total_frames = previous_frames.saturating_add(frames as u64);
        // This synthetic silence is not a device packet. Re-anchor when real
        // samples return, even if the device position stopped during playback idle.
        self.idle_end_ns = Some(origin_ns.saturating_add(self.duration_ns(total_frames)));
        self.idle_run = Some(IdleRun { origin_ns, frames: total_frames });
        PacketStamp {
            timestamp_ns: start,
            discontinuity,
        }
    }

    fn duration_ns(&self, frames: u64) -> u64 {
        u64::try_from(u128::from(frames) * 1_000_000_000 / u128::from(self.sample_rate))
            .unwrap_or(u64::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const START_NS: u64 = 1_000_000_000;
    const PACKET_NS: u64 = 10_000_000;
    const FRAMES: usize = 480;

    #[test]
    fn qpc_jitter_does_not_create_packet_scale_holes() {
        let mut clock = PacketClock::new(48_000);
        let mut previous = clock
            .timestamp(0, FRAMES, Some(START_NS), false, || unreachable!())
            .timestamp_ns;
        for packet in 1..100 {
            let jitter = if packet % 2 == 0 { 360_000 } else { -360_000 };
            let observation = (START_NS + packet * PACKET_NS).saturating_add_signed(jitter);
            let start = clock.timestamp(
                packet * FRAMES as u64,
                FRAMES,
                Some(observation),
                false,
                || unreachable!(),
            );
            assert!(!start.discontinuity);
            assert!((start.timestamp_ns - previous).abs_diff(PACKET_NS) <= 10_000);
            previous = start.timestamp_ns;
        }
    }

    #[test]
    fn genuine_device_frame_loss_remains_visible_even_without_a_qpc_reading() {
        let mut clock = PacketClock::new(48_000);
        clock.timestamp(0, FRAMES, Some(START_NS), false, || unreachable!());
        let gap = clock.timestamp(2 * FRAMES as u64, FRAMES, None, false, || unreachable!());
        assert!(gap.discontinuity);
        assert_eq!(gap.timestamp_ns, START_NS + 2 * PACKET_NS);
        assert_eq!(
            clock
                .timestamp(3 * FRAMES as u64, FRAMES, None, false, || unreachable!())
                .timestamp_ns,
            START_NS + 3 * PACKET_NS,
        );
    }

    #[test]
    fn discontinuity_accepts_a_reset_device_position() {
        let mut clock = PacketClock::new(48_000);
        clock.timestamp(960, FRAMES, Some(START_NS), false, || unreachable!());
        let next = START_NS + 4 * PACKET_NS;
        let gap = clock.timestamp(0, FRAMES, Some(next), true, || unreachable!());
        assert!(gap.discontinuity);
        assert_eq!(gap.timestamp_ns, next);
        assert_eq!(
            clock
                .timestamp(FRAMES as u64, FRAMES, None, false, || unreachable!())
                .timestamp_ns,
            next + PACKET_NS,
        );
    }

    #[test]
    fn idle_padding_does_not_reuse_a_stale_device_anchor() {
        let mut clock = PacketClock::new(48_000);
        clock.timestamp(0, FRAMES, Some(START_NS), false, || unreachable!());
        let idle = clock.idle_stamp(20 * FRAMES, || unreachable!());
        assert_eq!(idle.timestamp_ns, START_NS + PACKET_NS);
        assert!(idle.discontinuity);
        let wake = START_NS + 37 * PACKET_NS;
        let restart = clock.timestamp(FRAMES as u64, FRAMES, Some(wake), false, || unreachable!());
        assert_eq!(restart.timestamp_ns, wake);
        assert!(!restart.discontinuity);
    }

    #[test]
    fn idle_before_the_first_device_packet_does_not_report_a_discontinuity() {
        let mut clock = PacketClock::new(48_000);
        let idle = clock.idle_stamp(20 * FRAMES, || START_NS + 20 * PACKET_NS);
        assert_eq!(idle.timestamp_ns, START_NS);
        assert!(!idle.discontinuity);
    }

    #[test]
    fn repeated_idle_padding_continues_after_the_previous_synthetic_packet() {
        let mut clock = PacketClock::new(48_000);
        clock.timestamp(0, FRAMES, Some(START_NS), false, || unreachable!());
        let first = clock.idle_stamp(20 * FRAMES, || unreachable!());
        let next = clock.idle_stamp(20 * FRAMES, || unreachable!());
        assert_eq!(next.timestamp_ns, first.timestamp_ns + 20 * PACKET_NS);
        assert!(!next.discontinuity);
    }

    #[test]
    fn fractional_idle_periods_use_one_sample_grid_instead_of_accumulating_rounding() {
        let mut clock = PacketClock::new(44_100);
        clock.timestamp(0, 441, Some(START_NS), false, || unreachable!());
        for index in 0..300u64 {
            let idle = clock.idle_stamp(147, || unreachable!());
            assert_eq!(idle.timestamp_ns, START_NS + PACKET_NS + index * 1_000_000_000 / 300);
        }
        let next = clock.timestamp(441, 441, Some(START_NS + 101 * PACKET_NS), false, || unreachable!());
        assert_eq!(next.timestamp_ns, START_NS + 101 * PACKET_NS);
    }

    #[test]
    fn retained_idle_starts_at_enable_time_and_never_restores_the_prior_off_interval() {
        let mut clock = PacketClock::new(48_000);
        clock.timestamp(0, FRAMES, Some(START_NS), false, || unreachable!());
        let enable = START_NS + 60_000_000_000;
        assert!(clock.retained_idle_stamp(enable, 9_600).is_none());
        clock.set_retain_silence(true, enable);
        let (first, frames) = clock.retained_idle_stamp(enable + 1_000_000_000, 9_600).unwrap();
        assert_eq!(first.timestamp_ns, enable);
        assert_eq!(frames, 9_600);
        assert!(!first.discontinuity);
        clock.set_retain_silence(false, enable + 2_000_000_000);
        assert!(clock.retained_idle_stamp(enable + 3_000_000_000, 9_600).is_none());
        let next_enable = enable + 20_000_000_000;
        clock.set_retain_silence(true, next_enable);
        let (next, _) = clock.retained_idle_stamp(next_enable + 1_000_000_000, 9_600).unwrap();
        assert_eq!(next.timestamp_ns, next_enable);
    }

    #[test]
    fn retained_idle_is_bounded_and_does_not_advance_more_than_elapsed_time() {
        let mut clock = PacketClock::new(48_000);
        clock.set_retain_silence(true, START_NS);
        let until = START_NS + 1_000_000_000;
        for block in 0..10 {
            let (stamp, frames) = clock.retained_idle_stamp(until, 4_800).unwrap();
            assert_eq!(stamp.timestamp_ns, START_NS + block * 100_000_000);
            assert_eq!(frames, 4_800);
            assert!(!stamp.discontinuity);
        }
        assert!(clock.retained_idle_stamp(until, 4_800).is_none());
        assert!(clock.next.is_none(), "virtual silence must not advance a device position");
    }

    #[test]
    fn periodic_retained_idle_stays_on_the_44100_hz_frame_grid() {
        let mut clock = PacketClock::new(44_100);
        clock.set_retain_silence(true, START_NS);
        let mut total = 0u64;
        for tick in 1..=1_000 {
            let (stamp, frames) = clock.retained_idle_stamp(START_NS + tick * 1_000_000, 8_820).unwrap();
            assert_eq!(stamp.timestamp_ns, START_NS + total * 1_000_000_000 / 44_100);
            total += frames as u64;
        }
        assert_eq!(total, 44_100);
        assert_eq!(clock.idle_end_ns, Some(START_NS + 1_000_000_000));
    }

    #[test]
    fn retained_idle_reserves_an_inflight_packet_and_real_qpc_stays_unshifted() {
        let mut clock = PacketClock::new(48_000);
        clock.set_retain_silence(true, START_NS);
        clock.timestamp(0, FRAMES, Some(START_NS), false, || unreachable!());
        let now = START_NS + 1_000_000_000;
        let cutoff = clock.idle_cutoff_ns(now, 4_800, 4_000_000);
        assert_eq!(cutoff, now - 104_000_000);
        while clock.retained_idle_stamp(cutoff, 9_600).is_some() {}
        let virtual_end = clock.idle_end_ns.unwrap();
        let measured_start = now - PACKET_NS;
        assert!(virtual_end <= measured_start);
        let resumed = clock.timestamp(FRAMES as u64, FRAMES, Some(measured_start), false, || unreachable!());
        assert_eq!(resumed.timestamp_ns, measured_start);
        assert!(!resumed.discontinuity);
        let following = clock.timestamp(2 * FRAMES as u64, FRAMES, Some(now), false, || unreachable!());
        assert_eq!(following.timestamp_ns, now);
    }

    #[test]
    fn a_recreated_stream_restarts_retained_idle_at_resume_instead_of_filling_pause() {
        let mut clock = PacketClock::new(48_000);
        clock.set_retain_silence(true, START_NS);
        clock.retained_idle_stamp(START_NS + 1_000_000_000, 9_600).unwrap();
        let resume = START_NS + 30_000_000_000;
        clock = PacketClock::new(48_000);
        assert!(clock.retained_idle_stamp(resume + 1_000_000_000, 9_600).is_none());
        clock.set_retain_silence(true, resume);
        let (stamp, _) = clock.retained_idle_stamp(resume + 1_000_000_000, 9_600).unwrap();
        assert_eq!(stamp.timestamp_ns, resume);
        assert!(!stamp.discontinuity);
    }

    #[test]
    fn an_hour_of_known_idle_needs_only_an_anchor_and_one_current_packet() {
        let mut clock = PacketClock::new(44_100);
        clock.set_retain_silence(true, START_NS);
        let until = START_NS + 3_600_000_123_456;
        let (anchor, anchor_frames) = clock.retained_idle_stamp(until, 8_820).unwrap();
        assert_eq!(anchor.timestamp_ns, START_NS);
        assert_eq!(anchor_frames, 8_820);
        let (current, current_frames) = clock.retained_idle_stamp(until, 8_820).unwrap();
        let total_frames = (u128::from(until - START_NS) * 44_100 / 1_000_000_000) as u64;
        assert_eq!(current.timestamp_ns,
            START_NS + (total_frames - current_frames as u64) * 1_000_000_000 / 44_100);
        assert_eq!(current_frames, 8_820);
        assert!(!current.discontinuity);
        assert!(clock.retained_idle_stamp(until, 8_820).is_none());
        assert_eq!(clock.idle_end_ns, Some(START_NS + total_frames * 1_000_000_000 / 44_100));
    }

    #[test]
    fn retained_idle_never_hides_a_real_device_discontinuity() {
        let mut clock = PacketClock::new(48_000);
        clock.set_retain_silence(true, START_NS);
        clock.timestamp(0, FRAMES, Some(START_NS), false, || unreachable!());
        clock.retained_idle_stamp(START_NS + 500_000_000, 9_600).unwrap();
        let wake = START_NS + 1_000_000_000;
        let packet = clock.timestamp(0, FRAMES, Some(wake), true, || unreachable!());
        assert_eq!(packet.timestamp_ns, wake);
        assert!(packet.discontinuity);
        let following = clock.timestamp(FRAMES as u64, FRAMES, Some(wake + PACKET_NS), false, || unreachable!());
        assert_eq!(following.timestamp_ns, wake + PACKET_NS);
        assert!(!following.discontinuity);
    }

    #[test]
    fn virtual_zero_positions_still_accept_adjacent_timestamps() {
        let mut clock = PacketClock::new(48_000);
        clock.timestamp(0, FRAMES, Some(START_NS), false, || unreachable!());
        let observed = START_NS + PACKET_NS + 250_000;
        let next = clock.timestamp(0, FRAMES, Some(observed), false, || unreachable!());
        assert!(!next.discontinuity);
        assert!(next.timestamp_ns.abs_diff(START_NS + PACKET_NS) <= 10_000);
    }
}
