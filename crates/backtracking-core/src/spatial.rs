//! Stereo and binaural rendering shared by preview and export.
//!
//! Construct a head model and renderer off the audio thread. [`BinauralRenderer::render`]
//! accepts any number of interleaved input frames and writes the same number of stereo
//! frames without allocating. Its filter history and internal 128-frame filter cadence
//! survive calls, so the caller's buffer sizes do not change the result.
//!
//! Directions use degrees: zero azimuth is ahead, positive azimuth is right, and
//! positive elevation is up. LFE and unrecognised labels resolve to `None`: they go
//! equally to both ears at `1 / sqrt(2)`. Every binaural source receives a further
//! factor of 0.5, with no limiter or mix normalisation. Filter tails are cut at the
//! end of the supplied input. Apply export fades and sample-format conversion afterwards.

mod directions;
mod head_model;
mod renderer;

pub use directions::{fold_down, speaker_directions, stereo_weights, Direction};
pub use head_model::{HeadModel, HEAD_MODEL_DIRECTIONS, HEAD_MODEL_TABLE_BYTES};
pub use renderer::{render_binaural, BinauralRenderer, BinauralSource, PositionKey};

/// Invalid setup or buffers. A rejected render leaves both output and state unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderError {
    InvalidSampleRate,
    InvalidBuffer,
    InvalidSource,
    InvalidSelection,
    InvalidPositionKeys,
    TimelineOverflow,
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidSampleRate => "the sample rate must be between 8000 and 384000 Hz",
            Self::InvalidBuffer => "input and output must contain matching whole frames",
            Self::InvalidSource => {
                "sources must be nonempty and directions must be finite and valid"
            }
            Self::InvalidSelection => "selected channel indices must be unique and in range",
            Self::InvalidPositionKeys => {
                "position keys must be finite, valid, and strictly ordered"
            }
            Self::TimelineOverflow => "the render would overflow the frame timeline",
        })
    }
}

impl std::error::Error for RenderError {}

#[cfg(test)]
mod benchmark;
#[cfg(test)]
mod tests;
