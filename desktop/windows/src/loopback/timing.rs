struct PacketEnd {
    position: u64,
    timestamp_ns: u64,
    duration_ns: u64,
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
}

impl PacketClock {
    pub(super) fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            next: None,
            idle_end_ns: None,
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
                    (measured.max(minimum), idle_end_ns.is_none())
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
        let start = self.next.as_ref().map_or_else(
            || now_ns().saturating_sub(duration_ns),
            |next| next.timestamp_ns,
        );
        // This synthetic silence is not a device packet. Re-anchor when real
        // samples return, even if the device position stopped during playback idle.
        self.idle_end_ns = Some(start.saturating_add(duration_ns));
        PacketStamp {
            timestamp_ns: start,
            // The last real sample need not be close to zero when the endpoint stops.
            discontinuity: self.next.is_some(),
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
    fn virtual_zero_positions_still_accept_adjacent_timestamps() {
        let mut clock = PacketClock::new(48_000);
        clock.timestamp(0, FRAMES, Some(START_NS), false, || unreachable!());
        let observed = START_NS + PACKET_NS + 250_000;
        let next = clock.timestamp(0, FRAMES, Some(observed), false, || unreachable!());
        assert!(!next.discontinuity);
        assert!(next.timestamp_ns.abs_diff(START_NS + PACKET_NS) <= 10_000);
    }
}
