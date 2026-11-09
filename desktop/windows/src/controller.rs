use backtracking_core::{Capture, CaptureState, DurationChoice, MeterReadings, Snapshot};

use crate::loopback::Loopback;

pub(crate) struct Controller {
    loopback: Option<Loopback>,
    capture: Option<Capture>,
    scratch: Vec<f32>,
    retention: DurationChoice,
    format: Option<(u32, usize)>,
    revision: u64,
    error: Option<String>,
}

impl Controller {
    pub(crate) fn new() -> Self {
        let mut controller = Self {
            loopback: None,
            capture: None,
            scratch: vec![0.; 8192],
            retention: DurationChoice::Seconds20,
            format: None,
            revision: 0,
            error: None,
        };
        controller.resume();
        controller
    }

    pub(crate) fn poll(&mut self) {
        self.drain();
        if let Some(error) = self.loopback.as_ref().and_then(Loopback::take_error) {
            self.disconnect(error);
        }
    }

    fn drain(&mut self) {
        let (Some(loopback), Some(capture)) = (&self.loopback, &mut self.capture) else {
            return;
        };
        let before = capture.revision();
        for _ in 0..64 {
            let frames = loopback.queue.pop_interleaved(&mut self.scratch);
            if frames == 0 {
                break;
            }
            let _ = capture.push_interleaved(&self.scratch[..frames * loopback.channels]);
        }
        if capture.revision() != before {
            self.revision = self.revision.wrapping_add(1);
        }
    }

    fn disconnect(&mut self, error: String) {
        self.loopback = None;
        if let Some(capture) = &mut self.capture {
            capture.pause();
        }
        self.error = Some(error);
    }

    pub(crate) fn pause(&mut self) {
        if self.is_paused() {
            return;
        }
        let result = self.loopback.as_ref().map(Loopback::pause);
        self.drain();
        if let Some(capture) = &mut self.capture {
            capture.pause();
        }
        if let Some(Err(error)) = result {
            self.disconnect(error);
        }
    }

    pub(crate) fn resume(&mut self) {
        if !self.is_paused() {
            return;
        }
        if let Some(loopback) = &self.loopback {
            if let Err(error) = loopback.resume() {
                self.disconnect(error);
                return;
            }
        } else {
            let loopback = match Loopback::start() {
                Ok(loopback) => loopback,
                Err(error) => {
                    self.error = Some(error);
                    return;
                }
            };
            let format = (loopback.sample_rate, loopback.channels);
            if self.format != Some(format) {
                match Capture::new(format.0, format.1, self.retention) {
                    Ok(capture) => {
                        self.capture = Some(capture);
                        self.format = Some(format);
                        self.revision = self.revision.wrapping_add(1);
                    }
                    Err(error) => {
                        self.error = Some(error.to_string());
                        return;
                    }
                }
            }
            self.loopback = Some(loopback);
        }

        if let Some(capture) = &mut self.capture {
            capture.resume();
        }
        self.error = None;
    }

    pub(crate) fn is_paused(&self) -> bool {
        self.loopback.is_none()
            || self.capture.as_ref().map(Capture::state) == Some(CaptureState::Paused)
    }

    pub(crate) fn is_recording(&self) -> bool {
        self.capture.as_ref().map(Capture::state) == Some(CaptureState::Recording)
    }

    pub(crate) fn snapshot(&self) -> Option<Snapshot> {
        self.capture.as_ref().map(Capture::snapshot)
    }

    pub(crate) fn meter_readings(&self) -> Option<MeterReadings> {
        self.capture.as_ref().map(Capture::meter_readings)
    }

    pub(crate) fn clear(&mut self) {
        if let Some(loopback) = &self.loopback {
            loopback.queue.discard_pending();
        }
        if let Some(capture) = &mut self.capture {
            capture.clear();
            self.revision = self.revision.wrapping_add(1);
        }
    }

    pub(crate) fn waveform(&self, minimum: &mut [f32], maximum: &mut [f32]) -> (usize, u32) {
        let Some(capture) = &self.capture else {
            minimum.fill(0.);
            maximum.fill(0.);
            return (0, 48_000);
        };
        let rate = self.format.map_or(48_000, |format| format.0);
        (capture.waveform(minimum, maximum), rate)
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn retention(&self) -> DurationChoice {
        self.retention
    }

    pub(crate) fn set_retention(&mut self, retention: DurationChoice) {
        self.retention = retention;
        if let Some(capture) = &mut self.capture {
            let before = capture.revision();
            capture.set_retention(retention);
            if capture.revision() != before {
                self.revision = self.revision.wrapping_add(1);
            }
        }
    }

    pub(crate) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
}
