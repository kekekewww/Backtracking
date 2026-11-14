mod capture;
mod export;
mod ffi;
mod meter;
mod queue;
mod sender;
mod synchronized;
mod track_ffi;
pub mod waveform;

pub use capture::{
    Capture, CaptureError, CaptureState, DurationChoice, FrozenCapture, FrozenTrack, Snapshot,
};
pub use export::{
    export_routed_tracks_with_profile, export_routed_tracks_with_tempo,
    export_routed_wav_with_profile, export_tracks_with_profile, export_wav,
    export_wav_with_profile, write_wav, write_wav_with_profile, TrackAudio, WavExportProfile,
    WavSampleFormat,
};
pub use meter::MeterReadings;
pub use queue::{AudioQueue, TimedAudioQueue, TimedPacket};
pub use sender::{capture_clock_ns, discover_senders, discover_snapshot_targets, new_sender_id, request_sender_snapshot, SenderEndpoint, SenderInfo, SenderPacket, SenderReceiver};
pub use synchronized::{ChannelRoute, SynchronizedCapture};
pub use waveform::WaveformView;

use std::time::Duration;

/// Delay before a paused capture re-arms by itself after the last interaction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AutoResumeDelay {
    /// A paused capture stays paused until the user resumes it.
    Off,
    Seconds5,
    #[default]
    Seconds10,
}

impl AutoResumeDelay {
    pub const ALL: [Self; 3] = [Self::Off, Self::Seconds5, Self::Seconds10];

    /// Reads a stored preference. `0` has always meant that capture does not resume by itself.
    pub const fn from_seconds(seconds: u32) -> Option<Self> {
        match seconds {
            0 => Some(Self::Off),
            5 => Some(Self::Seconds5),
            10 => Some(Self::Seconds10),
            _ => None,
        }
    }

    /// The value written to preferences. [`Self::Off`] is stored as `0`.
    pub const fn seconds(self) -> u32 {
        match self {
            Self::Off => 0,
            Self::Seconds5 => 5,
            Self::Seconds10 => 10,
        }
    }

    /// How long a paused capture waits before resuming, or `None` when it never does.
    pub const fn delay(self) -> Option<Duration> {
        match self {
            Self::Off => None,
            Self::Seconds5 | Self::Seconds10 => Some(Duration::from_secs(self.seconds() as u64)),
        }
    }

    /// How much of the delay is left once `idle` has passed since the last interaction.
    /// `Some(Duration::ZERO)` means it is time to resume. `None` means capture stays paused.
    pub fn remaining(self, idle: Duration) -> Option<Duration> {
        self.delay().map(|delay| delay.saturating_sub(idle))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_resume_off_is_stored_as_zero_and_never_resumes() {
        for choice in AutoResumeDelay::ALL {
            assert_eq!(AutoResumeDelay::from_seconds(choice.seconds()), Some(choice));
        }
        assert_eq!(AutoResumeDelay::from_seconds(0), Some(AutoResumeDelay::Off));
        assert_eq!(AutoResumeDelay::Off.seconds(), 0);
        assert_eq!(AutoResumeDelay::from_seconds(3), None);
        assert_eq!(AutoResumeDelay::default(), AutoResumeDelay::Seconds10);

        assert_eq!(AutoResumeDelay::Off.delay(), None);
        assert_eq!(AutoResumeDelay::Off.remaining(Duration::from_secs(3_600)), None);
        let ten = AutoResumeDelay::Seconds10;
        assert_eq!(ten.delay(), Some(Duration::from_secs(10)));
        assert_eq!(ten.remaining(Duration::from_secs(3)), Some(Duration::from_secs(7)));
        assert_eq!(ten.remaining(Duration::from_secs(10)), Some(Duration::ZERO));
        assert_eq!(ten.remaining(Duration::from_secs(60)), Some(Duration::ZERO));
    }
}
