mod capture;
mod export;
mod ffi;
mod meter;
mod queue;
mod synchronized;
mod track_ffi;
pub mod waveform;

pub use capture::{Capture, CaptureError, CaptureState, DurationChoice, Snapshot};
pub use export::{
    export_tracks_with_profile, export_wav, export_wav_with_profile, write_wav,
    write_wav_with_profile, WavExportProfile, WavSampleFormat,
};
pub use meter::MeterReadings;
pub use queue::{AudioQueue, TimedAudioQueue, TimedPacket};
pub use synchronized::{ChannelRoute, SynchronizedCapture};
pub use waveform::WaveformView;

/// Delay before capture re-arms after an interaction with the waveform.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AutoResumeDelay {
    Immediate,
    Seconds5,
    #[default]
    Seconds10,
}

impl AutoResumeDelay {
    pub const ALL: [Self; 3] = [Self::Immediate, Self::Seconds5, Self::Seconds10];

    pub const fn from_seconds(seconds: u32) -> Option<Self> {
        match seconds {
            0 => Some(Self::Immediate),
            5 => Some(Self::Seconds5),
            10 => Some(Self::Seconds10),
            _ => None,
        }
    }

    pub const fn seconds(self) -> u32 {
        match self {
            Self::Immediate => 0,
            Self::Seconds5 => 5,
            Self::Seconds10 => 10,
        }
    }
}
