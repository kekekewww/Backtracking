use std::ops::Range;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use backtracking_core::{
    CaptureState, ChannelRoute, DurationChoice, MeterReadings, Snapshot, SynchronizedCapture,
    WaveformView,
};

pub(crate) use crate::loopback::CaptureSource;
use crate::loopback::{Format, Loopback, now_ns};

const COMMON_RATE: u32 = 48_000;
const RETRY_DELAY: Duration = Duration::from_secs(2);
const MAX_PACKETS_PER_POLL: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SourceSettings {
    pub(crate) source: CaptureSource,
    pub(crate) route: ChannelRoute,
}

impl Default for SourceSettings {
    fn default() -> Self {
        Self {
            source: CaptureSource::System,
            route: ChannelRoute::Merged,
        }
    }
}

struct Input {
    source: CaptureSource,
    stream: Loopback,
}

struct PendingInput {
    source: CaptureSource,
    result: mpsc::Receiver<Result<Loopback, String>>,
}

pub(crate) struct Controller {
    sources: [Option<SourceSettings>; 2],
    inputs: Vec<Input>,
    pending: Vec<PendingInput>,
    formats: [Option<Arc<Format>>; 2],
    statuses: [Option<String>; 2],
    capture: Option<SynchronizedCapture>,
    scratch: Vec<f32>,
    native: Vec<f32>,
    discard_before_ns: u64,
    retention: DurationChoice,
    shape: Option<(u32, usize, Option<usize>)>,
    generation: u64,
    revision: u64,
    paused: bool,
    next_retry: Instant,
    error: Option<String>,
}

impl Controller {
    pub(crate) fn new() -> Self {
        // The recorder loads the saved source preferences before starting capture.
        Self {
            sources: [Some(SourceSettings::default()), None],
            inputs: Vec::new(),
            pending: Vec::new(),
            formats: [None, None],
            statuses: [None, None],
            capture: None,
            scratch: Vec::new(),
            native: Vec::new(),
            discard_before_ns: 0,
            retention: DurationChoice::Seconds20,
            shape: None,
            generation: 0,
            revision: 0,
            paused: true,
            next_retry: Instant::now(),
            error: None,
        }
    }

    pub(crate) fn sources(&self) -> &[Option<SourceSettings>; 2] {
        &self.sources
    }

    pub(crate) fn set_sources(&mut self, mut sources: [Option<SourceSettings>; 2]) {
        if sources[0].is_none() {
            sources[0] = sources[1]
                .take()
                .or_else(|| Some(SourceSettings::default()));
        }
        if self.sources == sources {
            return;
        }
        let settings_changed =
            self.sources
                .iter()
                .zip(&sources)
                .any(|(previous, next)| match (previous, next) {
                    (Some(previous), Some(next)) => {
                        previous.route != next.route || !previous.source.same_source(&next.source)
                    }
                    (None, None) => false,
                    _ => true,
                });
        self.sources = sources;
        if !settings_changed {
            return;
        }

        self.inputs.retain(|input| {
            self.sources
                .iter()
                .flatten()
                .any(|lane| lane.source.same_source(&input.source))
        });
        self.pending.retain(|pending| {
            self.sources
                .iter()
                .flatten()
                .any(|lane| lane.source.same_source(&pending.source))
        });
        self.discard_before_ns = now_ns();
        for input in &self.inputs {
            input.stream.discard_pending(&mut self.native);
        }
        self.formats = std::array::from_fn(|lane| {
            let source = &self.sources[lane].as_ref()?.source;
            self.inputs
                .iter()
                .find(|input| input.source.same_source(source))
                .map(|input| Arc::clone(&input.stream.format))
        });
        self.statuses = std::array::from_fn(|lane| {
            let settings = self.sources[lane].as_ref()?;
            let format = self.formats[lane].as_ref()?;
            missing_channel(format, settings.route)
        });
        self.error = None;
        self.reset_capture();
        self.configure_capture();
        if !self.paused {
            self.request_missing();
        }
    }

    pub(crate) fn available_sources() -> Vec<CaptureSource> {
        Loopback::available_sources()
    }

    pub(crate) fn route_available(&self, lane: usize, route: ChannelRoute) -> bool {
        if let Some(format) = self.formats.get(lane).and_then(Option::as_ref) {
            return format.route_channels(route).is_some();
        }
        // Until a source opens, avoid rejecting a saved route that the device may support.
        route != ChannelRoute::Center
    }

    pub(crate) fn source_status(&self, lane: usize) -> Option<&str> {
        self.statuses.get(lane).and_then(Option::as_deref)
    }

    pub(crate) fn poll(&mut self) {
        if self.paused {
            return;
        }
        self.poll_pending();
        self.drain(false);
        let had_inputs = !self.inputs.is_empty();
        let mut index = 0;
        while index < self.inputs.len() {
            let Some(error) = self.inputs[index].stream.take_error() else {
                index += 1;
                continue;
            };
            let input = self.inputs.remove(index);
            self.set_source_status(&input.source, error);
        }
        if had_inputs
            && self.inputs.is_empty()
            && let Some(capture) = &mut self.capture
        {
            capture.drain(true);
            capture.reset_input();
            capture.capture_mut().pause();
            capture.capture_mut().resume();
            self.revision = self.revision.wrapping_add(1);
        }
        if Instant::now() >= self.next_retry {
            self.request_missing();
        }
        self.update_error();
    }

    fn request_missing(&mut self) {
        self.next_retry = Instant::now() + RETRY_DELAY;
        let sources: Vec<_> = self
            .sources
            .iter()
            .flatten()
            .map(|lane| lane.source.clone())
            .collect();
        for source in sources {
            if self
                .inputs
                .iter()
                .any(|input| input.source.same_source(&source))
                || self
                    .pending
                    .iter()
                    .any(|pending| pending.source.same_source(&source))
            {
                continue;
            }
            let selected = source.clone();
            let (sender, result) = mpsc::sync_channel(1);
            match thread::Builder::new()
                .name("backtracking-source-open".into())
                .spawn(move || {
                    let _ = sender.send(Loopback::start(selected));
                }) {
                Ok(_) => self.pending.push(PendingInput { source, result }),
                Err(error) => self.set_source_status(&source, error.to_string()),
            }
        }
    }

    fn poll_pending(&mut self) {
        let mut index = 0;
        while index < self.pending.len() {
            let result = match self.pending[index].result.try_recv() {
                Ok(result) => result,
                Err(mpsc::TryRecvError::Empty) => {
                    index += 1;
                    continue;
                }
                Err(mpsc::TryRecvError::Disconnected) => Err("音訊來源連線已中斷".into()),
            };
            let PendingInput { source, .. } = self.pending.remove(index);
            match result {
                Ok(stream) => {
                    for (lane, settings) in self.sources.iter().enumerate() {
                        if let Some(settings) = settings
                            && settings.source.same_source(&source)
                        {
                            self.formats[lane] = Some(Arc::clone(&stream.format));
                            self.statuses[lane] = missing_channel(&stream.format, settings.route);
                        }
                    }
                    self.inputs.push(Input { source, stream });
                    self.configure_capture();
                }
                Err(error) => self.set_source_status(&source, error),
            }
        }
    }

    fn set_source_status(&mut self, source: &CaptureSource, error: String) {
        for (lane, settings) in self.sources.iter().enumerate() {
            if settings
                .as_ref()
                .is_some_and(|settings| settings.source.same_source(source))
            {
                self.statuses[lane] = Some(error.clone());
            }
        }
    }

    fn update_error(&mut self) {
        let active = self.sources.iter().enumerate().any(|(lane, settings)| {
            settings.as_ref().is_some_and(|settings| {
                self.statuses[lane].is_none()
                    && self
                        .inputs
                        .iter()
                        .any(|input| input.source.same_source(&settings.source))
            })
        });
        self.error = if active || !self.pending.is_empty() {
            None
        } else {
            self.statuses.iter().flatten().next().cloned()
        };
    }

    fn configure_capture(&mut self) {
        if self.formats.iter().all(Option::is_none) {
            return;
        }
        let channels = |lane: usize| {
            let source = self.sources[lane].as_ref()?;
            Some(
                self.formats[lane]
                    .as_ref()
                    .and_then(|format| format.route_channels(source.route))
                    .unwrap_or_else(|| {
                        if source.route != ChannelRoute::Merged {
                            return 1;
                        }
                        match source.source {
                            CaptureSource::System | CaptureSource::Application { .. } => 2,
                            CaptureSource::DefaultMicrophone | CaptureSource::Microphone { .. } => {
                                1
                            }
                        }
                    }),
            )
        };
        let sample_rate = if self.sources[1].is_some() {
            COMMON_RATE
        } else {
            self.formats[0]
                .as_ref()
                .map_or(COMMON_RATE, |format| format.sample_rate)
        };
        let shape = (sample_rate, channels(0).unwrap_or(2), channels(1));
        if self.shape == Some(shape) {
            return;
        }
        match SynchronizedCapture::new(shape.0, shape.1, shape.2, self.retention) {
            Ok(mut capture) => {
                if self.paused {
                    capture.capture_mut().pause();
                }
                self.capture = Some(capture);
                self.shape = Some(shape);
                self.bump_generation();
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    fn reset_capture(&mut self) {
        self.capture = None;
        self.shape = None;
        self.bump_generation();
    }

    fn bump_generation(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.revision = self.revision.wrapping_add(1);
    }

    fn drain(&mut self, flush: bool) {
        let Some(capture) = &mut self.capture else {
            return;
        };
        let before = capture.capture().revision();
        // Drain one packet from each input per round, so a burst from one device
        // cannot commit silence over queued data from the other device.
        for _ in 0..MAX_PACKETS_PER_POLL {
            let mut found = false;
            for input in &self.inputs {
                let Some(packet) = input.stream.packet(&mut self.native) else {
                    continue;
                };
                found = true;
                let format = &input.stream.format;
                let delta = self.discard_before_ns.saturating_sub(packet.timestamp_ns);
                let skip = (u128::from(delta) * u128::from(packet.sample_rate))
                    .div_ceil(1_000_000_000) as usize;
                if skip >= packet.frames {
                    continue;
                }
                let samples = &self.native[skip * format.channels..];
                let timestamp_ns = packet.timestamp_ns.saturating_add(
                    (skip as u128 * 1_000_000_000 / u128::from(packet.sample_rate)) as u64,
                );
                for (lane, source) in self.sources.iter().enumerate() {
                    let Some(source) = source
                        .as_ref()
                        .filter(|source| source.source.same_source(&input.source))
                    else {
                        continue;
                    };
                    format.route(source.route, samples, &mut self.scratch);
                    if !self.scratch.is_empty()
                        && let Err(error) =
                            capture.push(lane, timestamp_ns, packet.sample_rate, &self.scratch)
                    {
                        self.statuses[lane] = Some(error.to_string());
                    }
                }
            }
            if !found {
                break;
            }
            capture.drain(false);
        }
        if flush {
            capture.drain(true);
        }
        if capture.capture().revision() != before {
            self.revision = self.revision.wrapping_add(1);
        }
    }

    pub(crate) fn pause(&mut self) {
        if self.paused {
            return;
        }
        self.paused = true;
        // Drop pending receivers so a connection that finishes after a pause shuts down.
        self.pending.clear();
        let mut index = 0;
        while index < self.inputs.len() {
            match self.inputs[index].stream.pause() {
                Ok(()) => index += 1,
                Err(error) => {
                    let input = self.inputs.remove(index);
                    self.set_source_status(&input.source, error);
                }
            }
        }
        self.drain(true);
        if let Some(capture) = &mut self.capture {
            capture.capture_mut().pause();
            capture.reset_input();
        }
        self.update_error();
    }

    pub(crate) fn resume(&mut self) {
        if !self.paused {
            if self.is_paused() {
                self.request_missing();
                self.update_error();
            }
            return;
        }
        self.paused = false;
        let mut index = 0;
        while index < self.inputs.len() {
            match self.inputs[index].stream.resume() {
                Ok(()) => index += 1,
                Err(error) => {
                    let input = self.inputs.remove(index);
                    self.set_source_status(&input.source, error);
                }
            }
        }
        if let Some(capture) = &mut self.capture {
            capture.reset_input();
            capture.capture_mut().resume();
        }
        self.request_missing();
        self.update_error();
    }

    pub(crate) fn is_paused(&self) -> bool {
        self.paused || (self.inputs.is_empty() && self.pending.is_empty())
    }

    pub(crate) fn is_recording(&self) -> bool {
        !self.paused
            && !self.inputs.is_empty()
            && self
                .capture
                .as_ref()
                .is_some_and(|capture| capture.capture().state() == CaptureState::Recording)
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn frame_range(&self) -> Range<u64> {
        self.capture
            .as_ref()
            .map_or(0..0, |capture| capture.capture().frame_range())
    }

    pub(crate) fn track_snapshot(&self, lane: usize) -> Option<Snapshot> {
        self.capture.as_ref()?.capture().track_snapshot(lane)
    }

    pub(crate) fn track_meter_readings(&self, lane: usize) -> Option<MeterReadings> {
        self.capture.as_ref()?.capture().track_meter_readings(lane)
    }

    pub(crate) fn track_channels(&self, lane: usize) -> Option<usize> {
        self.capture.as_ref()?.capture().track_channels(lane)
    }

    pub(crate) fn track_waveform(
        &self,
        lane: usize,
        view: WaveformView,
        minimum: &mut [f32],
        maximum: &mut [f32],
    ) -> (usize, u32) {
        let Some(capture) = &self.capture else {
            minimum.fill(0.);
            maximum.fill(0.);
            return (0, COMMON_RATE);
        };
        let rate = self.shape.map_or(COMMON_RATE, |shape| shape.0);
        (
            capture
                .capture()
                .track_waveform(lane, view, minimum, maximum),
            rate,
        )
    }

    pub(crate) fn clear(&mut self) {
        self.discard_before_ns = now_ns();
        for input in &self.inputs {
            input.stream.discard_pending(&mut self.native);
        }
        if let Some(capture) = &mut self.capture {
            capture.reset_input();
            capture.capture_mut().clear();
        }
        self.bump_generation();
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
            let capture = capture.capture_mut();
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

fn missing_channel(format: &Format, route: ChannelRoute) -> Option<String> {
    format
        .route_channels(route)
        .is_none()
        .then(|| "目前的來源沒有提供所選聲道".into())
}
