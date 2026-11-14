use std::io;
use std::ops::Range;
use std::sync::{Arc, Mutex, MutexGuard, TryLockError, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use backtracking_core::{
    CaptureError, CaptureState, ChannelRoute, DurationChoice, MeterReadings, SenderReceiver,
    Snapshot, SynchronizedCapture, TimedPacket, WaveformView, discover_senders,
};

pub(crate) use crate::loopback::CaptureSource;
use crate::loopback::{Format, Loopback, PACKET_QUEUE_CAPACITY, QuietModeBarrier, now_ns};

const COMMON_RATE: u32 = 48_000;
const RETRY_DELAY: Duration = Duration::from_secs(2);
const CAPTURE_POLL_INTERVAL: Duration = Duration::from_millis(4);

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
    stream: Stream,
}

struct PendingInput {
    source: CaptureSource,
    result: mpsc::Receiver<Result<Stream, String>>,
}

enum Stream {
    Wasapi(Loopback),
    Sender {
        receiver: Option<SenderReceiver>,
        format: Arc<Format>,
        id: String,
        error: Option<String>,
    },
}

impl Stream {
    fn open(source: CaptureSource) -> Result<Self, String> {
        match source {
            CaptureSource::Sender { id, .. } => {
                let receiver = SenderReceiver::connect(&id).map_err(|error| error.to_string())?;
                let format = Arc::new(Format::sender(
                    receiver.info.sample_rate,
                    receiver.info.channels,
                ));
                Ok(Self::Sender {
                    receiver: Some(receiver),
                    format,
                    id,
                    error: None,
                })
            }
            _ => Loopback::start(source).map(Self::Wasapi),
        }
    }

    fn format(&self) -> &Arc<Format> {
        match self {
            Self::Wasapi(stream) => &stream.format,
            Self::Sender { format, .. } => format,
        }
    }

    fn packet(&mut self, samples: &mut Vec<f32>, bars: &mut Vec<usize>) -> Option<TimedPacket> {
        bars.clear();
        match self {
            Self::Wasapi(stream) => stream.packet(samples),
            Self::Sender { receiver, error, .. } => match receiver.as_mut()?.packet(samples, bars) {
                Ok(packet) => packet.map(|packet| TimedPacket {
                    timestamp_ns: packet.timestamp_ns,
                    sample_rate: packet.sample_rate,
                    frames: packet.frames,
                    discontinuity: false,
                }),
                Err(problem) => {
                    *receiver = None;
                    *error = Some(problem.to_string());
                    None
                }
            },
        }
    }

    fn discard_pending(&mut self, samples: &mut Vec<f32>, bars: &mut Vec<usize>) {
        match self {
            Self::Wasapi(stream) => stream.discard_pending(samples),
            Self::Sender { .. } => {
                for _ in 0..PACKET_QUEUE_CAPACITY {
                    if self.packet(samples, bars).is_none() {
                        break;
                    }
                }
            }
        }
    }

    fn pause(&mut self) -> Result<(), String> {
        match self {
            Self::Wasapi(stream) => stream.pause(),
            Self::Sender { .. } => Ok(()),
        }
    }

    fn suspend(&mut self) {
        if let Self::Sender { receiver, .. } = self {
            *receiver = None;
        }
    }

    fn resume(&mut self) -> Result<(), String> {
        match self {
            Self::Wasapi(stream) => stream.resume(),
            Self::Sender {
                receiver, format, id, ..
            } => {
                let next = SenderReceiver::connect(id).map_err(|error| error.to_string())?;
                if next.info.sample_rate != format.sample_rate || next.info.channels != format.channels {
                    return Err("Sender format changed".into());
                }
                *receiver = Some(next);
                Ok(())
            }
        }
    }

    fn take_error(&mut self) -> Option<String> {
        match self {
            Self::Wasapi(stream) => stream.take_error(),
            Self::Sender { error, .. } => error.take(),
        }
    }

    fn set_retain_silence(&mut self, retain_silence: bool) -> Result<(), String> {
        match self {
            Self::Wasapi(stream) => stream.set_retain_silence(retain_silence),
            Self::Sender { .. } => Ok(()),
        }
    }

    fn prepare_retain_silence(
        &mut self,
        retain_silence: bool,
    ) -> Result<Option<QuietModeBarrier>, String> {
        match self {
            Self::Wasapi(stream) => stream.prepare_retain_silence(retain_silence).map(Some),
            Self::Sender { .. } => Ok(None),
        }
    }
}

type Sources = [Option<SourceSettings>; 2];
type WaveformBuffers<'a> = (&'a mut [f32], &'a mut [f32]);

pub(crate) struct EditSnapshot {
    pub(crate) snapshots: Arc<[Snapshot]>,
    pub(crate) bar_markers: Vec<u64>,
    pub(crate) generation: u64,
}

enum CaptureCommand {
    SetSources(Sources),
    SetRetention(DurationChoice),
    SetRetainSilence(bool),
    Pause,
    Resume,
    Clear,
    Snapshot {
        selection: Option<Range<u64>>,
        reply: mpsc::SyncSender<Option<Arc<[Snapshot]>>>,
    },
    EditSnapshot {
        reply: mpsc::SyncSender<Option<EditSnapshot>>,
    },
    Shutdown,
}

struct Request {
    command: CaptureCommand,
    complete: Option<mpsc::SyncSender<SourceStatus>>,
}

#[derive(Default)]
struct SourceStatus {
    formats: [Option<Arc<Format>>; 2],
    statuses: [Option<String>; 2],
    error: Option<String>,
    paused: bool,
    recording: bool,
    generation: u64,
    revision: u64,
    channels: [Option<usize>; 2],
    meters: [Option<MeterReadings>; 2],
}

impl SourceStatus {
    fn from_session(state: &CaptureSession) -> Self {
        let capture = state.capture.as_ref().map(SynchronizedCapture::capture);
        Self {
            formats: state.formats.clone(),
            statuses: state.statuses.clone(),
            error: state.error.clone().or_else(|| state.mode_error.clone()),
            paused: state.is_paused(),
            recording: state.is_recording(),
            generation: state.generation,
            revision: state.revision,
            channels: std::array::from_fn(|lane| {
                capture.and_then(|capture| capture.track_channels(lane))
            }),
            meters: std::array::from_fn(|lane| {
                capture.and_then(|capture| capture.track_meter_readings(lane))
            }),
        }
    }
}

pub(crate) struct Controller {
    sources: Sources,
    retention: DurationChoice,
    retain_silence: bool,
    source_status: SourceStatus,
    state: Arc<Mutex<CaptureSession>>,
    commands: mpsc::Sender<Request>,
    worker: Option<JoinHandle<()>>,
}

impl Controller {
    pub(crate) fn new() -> io::Result<Self> {
        let state = Arc::new(Mutex::new(CaptureSession::new()));
        let (commands, requests) = mpsc::channel();
        let worker_state = Arc::clone(&state);
        let worker = thread::Builder::new()
            .name("backtracking-capture".into())
            .spawn(move || CaptureSession::run(&worker_state, requests))?;
        Ok(Self {
            sources: [Some(SourceSettings::default()), None],
            retention: DurationChoice::Seconds20,
            retain_silence: false,
            source_status: SourceStatus {
                paused: true,
                ..SourceStatus::default()
            },
            state,
            commands,
            worker: Some(worker),
        })
    }

    pub(crate) fn sources(&self) -> &Sources {
        &self.sources
    }

    pub(crate) fn set_sources(&mut self, mut sources: Sources) {
        if sources[0].is_none() {
            sources[0] = sources[1].take().or_else(|| Some(SourceSettings::default()));
        }
        if self.sources != sources {
            self.sources = sources.clone();
            self.request(CaptureCommand::SetSources(sources));
            // A new source can recover from an earlier policy-control failure.
            let _ = self.commands.send(Request {
                command: CaptureCommand::SetRetainSilence(self.retain_silence),
                complete: None,
            });
        }
    }

    pub(crate) fn available_sources() -> Vec<CaptureSource> {
        let mut sources = Loopback::available_sources();
        sources.extend(discover_senders().into_iter().map(|sender| CaptureSource::Sender {
            id: sender.id,
            name: format!("【Sender】{}", sender.name),
        }));
        sources
    }

    pub(crate) fn route_available(&self, lane: usize, route: ChannelRoute) -> bool {
        if let Some(format) = self.source_status.formats.get(lane).and_then(Option::as_ref) {
            return format.route_channels(route).is_some();
        }
        // Until a source opens, avoid rejecting a saved route that the device may support.
        route != ChannelRoute::Center
    }

    pub(crate) fn source_status(&self, lane: usize) -> Option<&str> {
        self.source_status.statuses.get(lane).and_then(Option::as_deref)
    }

    pub(crate) fn poll(&mut self) {
        // Audio work can include a cold allocation, page release, or a snapshot
        // copy. A paint must retain the last complete metadata instead of
        // waiting for that work on the UI thread.
        if let Some(state) = try_lock(&self.state) {
            self.source_status = SourceStatus::from_session(&state);
        }
    }

    pub(crate) fn pause(&mut self) {
        self.request(CaptureCommand::Pause);
    }

    pub(crate) fn resume(&mut self) {
        self.request(CaptureCommand::Resume);
    }

    pub(crate) fn clear(&mut self) {
        self.request(CaptureCommand::Clear);
    }

    pub(crate) fn is_paused(&self) -> bool {
        self.source_status.paused
    }

    pub(crate) fn is_recording(&self) -> bool {
        self.source_status.recording
    }

    pub(crate) fn generation(&self) -> u64 {
        self.source_status.generation
    }

    #[cfg(test)]
    pub(crate) fn snapshots(&self, selection: Option<Range<u64>>) -> Option<Arc<[Snapshot]>> {
        self.request_snapshots(selection).recv().ok().flatten()
    }

    /// Starts a snapshot copy on the capture worker. Receiving its result is
    /// optional and must be polled, rather than waited for, from the UI thread.
    pub(crate) fn request_snapshots(
        &self,
        selection: Option<Range<u64>>,
    ) -> mpsc::Receiver<Option<Arc<[Snapshot]>>> {
        let (reply, result) = mpsc::sync_channel(1);
        let _ = self.commands.send(Request {
            command: CaptureCommand::Snapshot { selection, reply },
            complete: None,
        });
        result
    }

    /// Pins both audio lanes and the complete matching bar history in one
    /// worker transaction, independently of the last visible waveform zoom.
    pub(crate) fn request_edit_snapshot(&self) -> mpsc::Receiver<Option<EditSnapshot>> {
        let (reply, result) = mpsc::sync_channel(1);
        let _ = self.commands.send(Request {
            command: CaptureCommand::EditSnapshot { reply },
            complete: None,
        });
        result
    }

    pub(crate) fn track_meter_readings(&self, lane: usize) -> Option<MeterReadings> {
        self.source_status.meters.get(lane).copied().flatten()
    }

    pub(crate) fn track_channels(&self, lane: usize) -> Option<usize> {
        self.source_status.channels.get(lane).copied().flatten()
    }

    #[cfg(test)]
    fn render_waveforms(
        &self,
        views: &[(usize, WaveformView)],
        zoom: Option<Range<u64>>,
        traces: &mut [WaveformBuffers<'_>],
    ) -> (Range<u64>, Range<u64>, u32, Vec<u64>) {
        let state = lock(&self.state);
        Self::render_waveforms_from(&state, views, zoom, traces)
    }

    /// Leaves every output buffer untouched when the capture worker is busy.
    /// The caller can retain its previous waveform and selection for this frame.
    pub(crate) fn try_render_waveforms(
        &self,
        views: &[(usize, WaveformView)],
        zoom: Option<Range<u64>>,
        traces: &mut [WaveformBuffers<'_>],
    ) -> Option<(Range<u64>, Range<u64>, u32, Vec<u64>)> {
        let state = try_lock(&self.state)?;
        Some(Self::render_waveforms_from(&state, views, zoom, traces))
    }

    fn render_waveforms_from(
        state: &CaptureSession,
        views: &[(usize, WaveformView)],
        zoom: Option<Range<u64>>,
        traces: &mut [WaveformBuffers<'_>],
    ) -> (Range<u64>, Range<u64>, u32, Vec<u64>) {
        let rate = state.shape.map_or(COMMON_RATE, |shape| shape.0);
        let Some(capture) = state.capture.as_ref().map(SynchronizedCapture::capture) else {
            for (minimum, maximum) in traces {
                minimum.fill(0.0);
                maximum.fill(0.0);
            }
            return (0..0, 0..0, rate, Vec::new());
        };
        let history = capture.frame_range();
        let visible = zoom.map_or_else(
            || history.clone(),
            |zoom| {
                super::ui::fit_frame_range(
                    &history,
                    zoom.start,
                    zoom.end.saturating_sub(zoom.start),
                )
            },
        );
        for (&(lane, view), (minimum, maximum)) in views.iter().zip(traces) {
            capture.track_waveform_range(lane, view, visible.clone(), minimum, maximum);
        }
        let bars = capture.bar_markers(visible.clone());
        (history, visible, rate, bars)
    }

    pub(crate) fn revision(&self) -> u64 {
        self.source_status.revision
    }

    pub(crate) fn retention(&self) -> DurationChoice {
        self.retention
    }

    pub(crate) fn set_retention(&mut self, retention: DurationChoice) {
        self.retention = retention;
        let _ = self.commands.send(Request {
            command: CaptureCommand::SetRetention(retention),
            complete: None,
        });
    }

    pub(crate) fn retain_silence(&self) -> bool {
        self.retain_silence
    }

    pub(crate) fn set_retain_silence(&mut self, retain_silence: bool) {
        if self.retain_silence == retain_silence {
            return;
        }
        self.retain_silence = retain_silence;
        let _ = self.commands.send(Request {
            command: CaptureCommand::SetRetainSilence(retain_silence),
            complete: None,
        });
    }

    pub(crate) fn error(&self) -> Option<&str> {
        self.source_status.error.as_deref()
    }

    fn request(&mut self, command: CaptureCommand) {
        let (reply, result) = mpsc::sync_channel(1);
        if self
            .commands
            .send(Request {
                command,
                complete: Some(reply),
            })
            .is_ok()
        {
            if let Ok(status) = result.recv() {
                self.source_status = status;
            }
        }
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        let _ = self.commands.send(Request {
            command: CaptureCommand::Shutdown,
            complete: None,
        });
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn lock(state: &Mutex<CaptureSession>) -> MutexGuard<'_, CaptureSession> {
    state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn try_lock(state: &Mutex<CaptureSession>) -> Option<MutexGuard<'_, CaptureSession>> {
    match state.try_lock() {
        Ok(state) => Some(state),
        Err(TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
        Err(TryLockError::WouldBlock) => None,
    }
}

struct CaptureSession {
    sources: [Option<SourceSettings>; 2],
    inputs: Vec<Input>,
    pending: Vec<PendingInput>,
    formats: [Option<Arc<Format>>; 2],
    statuses: [Option<String>; 2],
    capture: Option<SynchronizedCapture>,
    scratch: Vec<f32>,
    native: Vec<f32>,
    bars: Vec<usize>,
    discard_before_ns: u64,
    retention: DurationChoice,
    retain_silence: bool,
    requested_retain_silence: bool,
    shape: Option<(u32, usize, Option<usize>)>,
    generation: u64,
    revision: u64,
    paused: bool,
    next_retry: Instant,
    error: Option<String>,
    mode_error: Option<String>,
    next_mode_retry: Instant,
}

impl CaptureSession {
    fn new() -> Self {
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
            bars: Vec::new(),
            discard_before_ns: 0,
            retention: DurationChoice::Seconds20,
            retain_silence: false,
            requested_retain_silence: false,
            shape: None,
            generation: 0,
            revision: 0,
            paused: true,
            next_retry: Instant::now(),
            error: None,
            mode_error: None,
            next_mode_retry: Instant::now(),
        }
    }

    fn run(state: &Mutex<Self>, requests: mpsc::Receiver<Request>) {
        let mut paused = true;
        loop {
            let request = if paused {
                requests
                    .recv()
                    .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
            } else {
                requests.recv_timeout(CAPTURE_POLL_INTERVAL)
            };
            let mut state = lock(state);
            match request {
                Ok(Request { command, complete }) => {
                    match command {
                        CaptureCommand::SetSources(sources) => state.set_sources(sources),
                        CaptureCommand::SetRetention(retention) => state.set_retention(retention),
                        CaptureCommand::SetRetainSilence(retain_silence) => {
                            state.set_retain_silence(retain_silence);
                        }
                        CaptureCommand::Pause => state.pause(),
                        CaptureCommand::Resume => state.resume(),
                        CaptureCommand::Clear => state.clear(),
                        CaptureCommand::Snapshot { selection, reply } => {
                            state.poll();
                            let _ = reply.send(state.snapshots(selection));
                        }
                        CaptureCommand::EditSnapshot { reply } => {
                            state.poll();
                            let _ = reply.send(state.edit_snapshot());
                        }
                        CaptureCommand::Shutdown => break,
                    }
                    if let Some(complete) = complete {
                        let _ = complete.send(SourceStatus::from_session(&state));
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => state.poll(),
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            paused = state.paused;
        }
    }

    fn set_sources(&mut self, sources: Sources) {
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
        for input in &mut self.inputs {
            input.stream.discard_pending(&mut self.native, &mut self.bars);
        }
        self.formats = std::array::from_fn(|lane| {
            let source = &self.sources[lane].as_ref()?.source;
            self.inputs
                .iter()
                .find(|input| input.source.same_source(source))
                .map(|input| Arc::clone(input.stream.format()))
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

    fn poll(&mut self) {
        if self.paused {
            return;
        }
        self.poll_pending();
        if self.requested_retain_silence != self.retain_silence
            && Instant::now() >= self.next_mode_retry
        {
            self.set_retain_silence(self.requested_retain_silence);
        }
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
            capture.capture_mut().smooth_next_join();
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
                    let _ = sender.send(Stream::open(selected));
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
                Ok(mut stream) => {
                    if let Err(error) = stream.set_retain_silence(self.retain_silence) {
                        self.set_source_status(&source, error);
                        continue;
                    }
                    for (lane, settings) in self.sources.iter().enumerate() {
                        if let Some(settings) = settings
                            && settings.source.same_source(&source)
                        {
                            self.formats[lane] = Some(Arc::clone(stream.format()));
                            self.statuses[lane] = missing_channel(stream.format(), settings.route);
                        }
                    }
                    self.inputs.push(Input { source, stream });
                    self.configure_capture();
                    if self.requested_retain_silence != self.retain_silence {
                        self.next_mode_retry = Instant::now();
                    }
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
                            CaptureSource::System
                            | CaptureSource::Application { .. }
                            | CaptureSource::Sender { .. } => 2,
                            CaptureSource::DefaultMicrophone | CaptureSource::Microphone { .. } => {
                                1
                            }
                        }
                    }),
            )
        };
        let sample_rate = if self.shared_source_routes().is_some() {
            self.formats[0]
                .as_ref()
                .map_or(COMMON_RATE, |format| format.sample_rate)
        } else {
            COMMON_RATE
        };
        let mut shape = (sample_rate, channels(0).unwrap_or(2), channels(1));
        if self.shape == Some(shape) {
            return;
        }
        let mut capture = SynchronizedCapture::new(shape.0, shape.1, shape.2, self.retention);
        if matches!(
            capture,
            Err(CaptureError::BufferTooLarge | CaptureError::AllocationFailed)
        ) && shape.0 != COMMON_RATE
        {
            // Very high native rates can exceed the maximum history allocation.
            shape.0 = COMMON_RATE;
            if self.shape == Some(shape) {
                return;
            }
            capture = SynchronizedCapture::new(shape.0, shape.1, shape.2, self.retention);
        }
        match capture {
            Ok(mut capture) => {
                capture.set_retain_silence(self.retain_silence);
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

    fn shared_source_routes(&self) -> Option<([ChannelRoute; 2], usize)> {
        let a = self.sources[0].as_ref()?;
        match &self.sources[1] {
            None => Some(([a.route, a.route], 1)),
            Some(b) if a.source.same_source(&b.source) => Some(([a.route, b.route], 2)),
            Some(_) => None,
        }
    }

    fn drain(&mut self, flush: bool) {
        let shared_routes = self.shared_source_routes();
        let capture_rate = self.shape.map_or(COMMON_RATE, |shape| shape.0);
        let Some(capture) = &mut self.capture else {
            return;
        };
        let before = capture.capture().revision();
        // Drain one packet from each input per round, so a burst from one device
        // cannot commit silence over queued data from the other device.
        for _ in 0..PACKET_QUEUE_CAPACITY {
            let mut found = false;
            for input in &mut self.inputs {
                let Some(packet) = input.stream.packet(&mut self.native, &mut self.bars) else {
                    continue;
                };
                found = true;
                let format = input.stream.format();
                let delta = self.discard_before_ns.saturating_sub(packet.timestamp_ns);
                let skip = (u128::from(delta) * u128::from(packet.sample_rate))
                    .div_ceil(1_000_000_000) as usize;
                if skip >= packet.frames {
                    continue;
                }
                let samples = &self.native[skip * format.channels..];
                self.bars.retain(|&marker| marker >= skip);
                for marker in &mut self.bars {
                    *marker -= skip;
                }
                let timestamp_ns = packet.timestamp_ns.saturating_add(
                    (skip as u128 * 1_000_000_000 / u128::from(packet.sample_rate)) as u64,
                );
                if let Some((routes, count)) = shared_routes
                    && packet.sample_rate == capture_rate
                {
                    // Both routes came from the same hardware frame and can stay on
                    // its native clock. WAV export converts the complete selection once.
                    format.route_tracks(&routes[..count], samples, &mut self.scratch);
                    if let Err(error) = capture.push_native_interleaved_with_markers(
                        timestamp_ns,
                        &self.scratch,
                        packet.discontinuity,
                        &self.bars,
                    ) {
                        for status in &mut self.statuses[..count] {
                            *status = Some(error.to_string());
                        }
                    }
                    continue;
                }
                for (lane, source) in self.sources.iter().enumerate() {
                    let Some(source) = source
                        .as_ref()
                        .filter(|source| source.source.same_source(&input.source))
                    else {
                        continue;
                    };
                    format.route(source.route, samples, &mut self.scratch);
                    if !self.scratch.is_empty()
                        && let Err(error) = capture.push_with_markers(
                            lane,
                            timestamp_ns,
                            packet.sample_rate,
                            &self.scratch,
                            packet.discontinuity,
                            &self.bars,
                        )
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
        if let Some(error) = capture.take_capture_error() {
            for (source, status) in self.sources.iter().zip(&mut self.statuses) {
                if source.is_some() {
                    *status = Some(error.to_string());
                }
            }
        }
        if capture.capture().revision() != before {
            self.revision = self.revision.wrapping_add(1);
        }
    }

    fn pause(&mut self) {
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
        for input in &mut self.inputs {
            input.stream.suspend();
        }
        if let Some(capture) = &mut self.capture {
            capture.capture_mut().pause();
            capture.reset_input();
        }
        self.update_error();
    }

    fn resume(&mut self) {
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
            capture.capture_mut().smooth_next_join();
            capture.capture_mut().resume();
        }
        self.request_missing();
        self.update_error();
    }

    fn is_paused(&self) -> bool {
        self.paused || (self.inputs.is_empty() && self.pending.is_empty())
    }

    fn is_recording(&self) -> bool {
        !self.paused
            && !self.inputs.is_empty()
            && self
                .capture
                .as_ref()
                .is_some_and(|capture| capture.capture().state() == CaptureState::Recording)
    }

    fn snapshots(&self, selection: Option<Range<u64>>) -> Option<Arc<[Snapshot]>> {
        let capture = self.capture.as_ref()?.capture();
        let frames = selection.unwrap_or_else(|| capture.frame_range());
        if frames.is_empty() {
            return None;
        }
        let count = if self.sources[1].is_some() { 2 } else { 1 };
        let snapshots = (0..count)
            .map(|lane| capture.track_snapshot_range(lane, frames.clone()))
            .collect::<Option<Vec<_>>>()?;
        Some(snapshots.into())
    }

    fn edit_snapshot(&self) -> Option<EditSnapshot> {
        let capture = self.capture.as_ref()?.capture();
        let frames = capture.frame_range();
        Some(EditSnapshot {
            snapshots: self.snapshots(Some(frames.clone()))?,
            bar_markers: capture.bar_markers(frames),
            generation: self.generation,
        })
    }

    fn clear(&mut self) {
        self.discard_before_ns = now_ns();
        for input in &mut self.inputs {
            input.stream.discard_pending(&mut self.native, &mut self.bars);
        }
        if let Some(capture) = &mut self.capture {
            capture.reset_input();
            capture.capture_mut().clear();
        }
        self.bump_generation();
    }

    fn set_retention(&mut self, retention: DurationChoice) {
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

    fn set_retain_silence(&mut self, retain_silence: bool) {
        self.requested_retain_silence = retain_silence;
        if self.retain_silence == retain_silence {
            self.mode_error = None;
            return;
        }
        // Freeze native synthetic padding before draining. Real packets keep
        // flowing, but no old idle tail can cross the capture-policy boundary.
        let mut barriers = Vec::new();
        let mut failures = Vec::new();
        for input in &mut self.inputs {
            match input.stream.prepare_retain_silence(retain_silence) {
                Ok(Some(barrier)) => barriers.push(barrier),
                Ok(None) => {}
                Err(error) => failures.push((input.source.clone(), error)),
            }
        }
        if !failures.is_empty() {
            for barrier in barriers {
                barrier.cancel();
            }
            self.mode_error = failures.into_iter().next()
                .map(|(_, error)| format!("Could not change quiet-section capture: {error}"));
            self.next_mode_retry = Instant::now() + RETRY_DELAY;
            return;
        }
        self.drain(true);
        self.retain_silence = retain_silence;
        if let Some(capture) = &mut self.capture {
            capture.set_retain_silence(retain_silence);
        }
        drop(barriers);
        self.mode_error = None;
    }
}

fn missing_channel(format: &Format, route: ChannelRoute) -> Option<String> {
    format
        .route_channels(route)
        .is_none()
        .then(|| "目前的來源沒有提供所選聲道".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_native_condition(
        controller: &mut Controller,
        timeout: Duration,
        mut ready: impl FnMut(&Controller) -> bool,
    ) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            controller.poll();
            if ready(controller) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn native_recorded_end(controller: &Controller) -> u64 {
        lock(&controller.state).capture.as_ref()
            .map_or(0, |capture| capture.capture().frame_range().end)
    }

    #[test]
    #[ignore = "native Process Loopback smoke; requires an isolated silent backtracking-quiet fixture PID"]
    fn native_quiet_retention_process_loopback_smoke() {
        use std::path::PathBuf;
        use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_TIMEOUT};
        use windows::Win32::System::Threading::{
            OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
            PROCESS_SYNCHRONIZE, QueryFullProcessImageNameW, WaitForSingleObject,
        };
        use windows::core::PWSTR;

        struct FixtureHandle(HANDLE);
        impl Drop for FixtureHandle {
            fn drop(&mut self) { let _ = unsafe { CloseHandle(self.0) }; }
        }

        let pid: u32 = std::env::var("BACKTRACKING_QUIET_TEST_PROCESS_ID")
            .expect("set BACKTRACKING_QUIET_TEST_PROCESS_ID to the isolated silent fixture")
            .parse().expect("fixture PID must be an integer");
        assert!(pid != 0 && pid != std::process::id());
        assert!(CaptureSource::supports_app_capture(), "Process Loopback is unsupported");
        let handle = FixtureHandle(unsafe {
            OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, false, pid)
        }.expect("cannot query the exact fixture process"));
        assert_eq!(unsafe { WaitForSingleObject(handle.0, 0) }, WAIT_TIMEOUT);
        let mut name = vec![0u16; 32_768];
        let mut length = name.len() as u32;
        unsafe {
            QueryFullProcessImageNameW(handle.0, PROCESS_NAME_WIN32, PWSTR(name.as_mut_ptr()), &mut length)
        }.expect("cannot resolve the exact fixture executable");
        let path = PathBuf::from(String::from_utf16(&name[..length as usize]).unwrap());
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent().unwrap().parent().unwrap().to_path_buf();
        assert!(path.canonicalize().unwrap().starts_with(workspace.join("target").canonicalize().unwrap()),
            "the native test may only capture a fixture under the workspace target directory");
        assert!(path.file_name().unwrap().to_string_lossy().to_ascii_lowercase()
            .starts_with("backtracking-quiet"), "unexpected fixture executable");
        let path = path.to_string_lossy().into_owned();
        assert_eq!(crate::loopback::resolved_process_id_for_test(&path).unwrap(), pid,
            "Application source resolution selected a different process");
        eprintln!("native quiet smoke: fixture identity verified");

        let source = CaptureSource::Application { path, name: "Quiet native fixture".into() };
        let mut controller = Controller::new().unwrap();
        controller.set_sources([Some(SourceSettings { source: source.clone(), route: ChannelRoute::Merged }), None]);
        controller.resume();
        eprintln!("native quiet smoke: dedicated Process Loopback activation requested");
        assert!(wait_native_condition(&mut controller, Duration::from_secs(8), |controller| {
            let state = lock(&controller.state);
            state.capture.is_some() && state.inputs.len() == 1
                && state.inputs[0].source.same_source(&source)
                && matches!(state.inputs[0].stream, Stream::Wasapi(_))
        }), "could not activate the isolated fixture: {:?}", controller.error());
        eprintln!("native quiet smoke: dedicated Process Loopback activated");
        assert!(controller.sources()[0].as_ref().unwrap().source.same_source(&source));
        let rate = lock(&controller.state).shape.unwrap().0;

        thread::sleep(Duration::from_millis(900));
        assert_eq!(native_recorded_end(&controller), 0, "default OFF retained idle audio");
        eprintln!("native quiet smoke: default OFF stable");
        controller.set_retain_silence(true);
        assert!(wait_native_condition(&mut controller, Duration::from_secs(2), |controller|
            lock(&controller.state).retain_silence));
        let on_start = native_recorded_end(&controller);
        let on_clock = Instant::now();
        assert!(wait_native_condition(&mut controller, Duration::from_secs(6), |controller|
            native_recorded_end(controller) >= on_start + u64::from(rate) * 3 / 5),
            "quiet ON did not advance the connected, silent Process Loopback stream");
        let on_elapsed = on_clock.elapsed().as_secs_f64();
        let on_end = native_recorded_end(&controller);
        eprintln!("native quiet smoke: quiet ON advances");
        assert!((on_end - on_start) as f64 <= (on_elapsed + 0.3) * f64::from(rate),
            "quiet ON retroactively retained the earlier OFF idle interval");
        let pinned = controller.request_edit_snapshot().recv().unwrap().unwrap();
        let pinned_range = pinned.snapshots[0].frame_range();
        assert!(pinned_range.end >= on_end);
        assert!(pinned.snapshots.iter().flat_map(|snapshot| snapshot.samples.iter()).all(|&sample| sample == 0.0),
            "the isolated silent fixture unexpectedly produced nonzero audio");

        controller.set_retain_silence(false);
        assert!(wait_native_condition(&mut controller, Duration::from_secs(2), |controller|
            !lock(&controller.state).retain_silence));
        let off_end = native_recorded_end(&controller);
        thread::sleep(Duration::from_millis(700));
        assert_eq!(native_recorded_end(&controller), off_end, "quiet OFF kept advancing");
        eprintln!("native quiet smoke: quiet OFF stable");

        controller.pause();
        controller.set_retain_silence(true);
        assert!(wait_native_condition(&mut controller, Duration::from_secs(2), |controller|
            lock(&controller.state).retain_silence));
        let paused_end = native_recorded_end(&controller);
        thread::sleep(Duration::from_millis(650));
        assert_eq!(native_recorded_end(&controller), paused_end, "manual Pause recorded quiet audio");
        assert!(controller.is_paused());
        eprintln!("native quiet smoke: manual Pause stable with quiet ON");
        controller.resume();
        let resume_clock = Instant::now();
        assert!(wait_native_condition(&mut controller, Duration::from_secs(6), |controller|
            native_recorded_end(controller) >= paused_end + u64::from(rate) / 4),
            "quiet recording did not continue after Resume");
        let resumed_frames = native_recorded_end(&controller) - paused_end;
        let resume_elapsed = resume_clock.elapsed().as_secs_f64();
        assert!(resumed_frames as f64 <= (resume_elapsed + 0.3) * f64::from(rate),
            "Resume backfilled the manually paused interval");
        assert_eq!(pinned.snapshots[0].frame_range(), pinned_range);
        assert_eq!(unsafe { WaitForSingleObject(handle.0, 0) }, WAIT_TIMEOUT);
        eprintln!("native quiet smoke: Resume advances without paused backfill");
        println!("{}", serde_json::json!({
            "scope": "isolated silent Process Loopback; memory snapshots only",
            "sample_rate": rate, "quiet_on_frames": on_end - on_start,
            "quiet_on_seconds_observed": on_elapsed, "quiet_off_end": off_end,
            "paused_end": paused_end, "resumed_frames": resumed_frames,
            "resume_seconds_observed": resume_elapsed,
        }));
    }

    #[test]
    fn quiet_retention_command_returns_while_capture_work_is_busy() {
        let controller = Controller::new().unwrap();
        assert!(!controller.retain_silence());
        let state = Arc::clone(&controller.state);
        let busy = lock(&state);
        let (posted, result) = mpsc::sync_channel(1);
        let ui = thread::spawn(move || {
            let mut controller = controller;
            controller.set_retain_silence(true);
            assert!(controller.retain_silence());
            posted.send(controller).unwrap();
        });
        let response = result.recv_timeout(Duration::from_millis(100));
        let returned_while_busy = response.is_ok();
        drop(busy);
        ui.join().unwrap();
        assert!(returned_while_busy, "quiet retention waited for the capture mutex");
        let controller = response.unwrap();
        assert!(controller.request_snapshots(None).recv().unwrap().is_none());
        assert!(lock(&state).retain_silence);
    }

    #[test]
    fn quiet_retention_switch_preserves_a_b_audio_and_an_existing_edit() {
        let mut controller = Controller::new().unwrap();
        {
            let mut session = lock(&controller.state);
            session.sources[1] = Some(SourceSettings::default());
            session.shape = Some((10, 1, Some(1)));
            session.generation = 7;
            session.paused = false;
            session.next_retry = Instant::now() + Duration::from_secs(3600);
            let mut capture =
                SynchronizedCapture::new(10, 1, Some(1), DurationChoice::Seconds20).unwrap();
            capture.push_native_interleaved_with_markers(
                0, &vec![0.25, -0.5].repeat(6), false, &[1],
            ).unwrap();
            session.capture = Some(capture);
        }
        let edit = controller.request_edit_snapshot().recv().unwrap().unwrap();
        assert_eq!(edit.snapshots[0].frame_range(), 0..6);
        controller.set_retain_silence(true);
        controller.request_snapshots(None).recv().unwrap();
        {
            let mut session = lock(&controller.state);
            assert!(session.retain_silence);
            assert!(!session.paused);
            assert_eq!(session.generation, 7);
            let capture = session.capture.as_mut().unwrap();
            assert!(capture.retain_silence());
            assert_eq!(capture.capture().state(), CaptureState::Recording);
            capture.push_native_interleaved_with_markers(
                600_000_000, &[0.0; 20], false, &[2, 7],
            ).unwrap();
        }
        let live = controller.snapshots(None).unwrap();
        assert!(live.iter().all(|snapshot| snapshot.frame_range() == (0..16)));
        assert_eq!(&live[0].samples[..6], &[0.25; 6]);
        assert_eq!(&live[1].samples[..6], &[-0.5; 6]);
        assert_eq!(&live[0].samples[6..], &[0.0; 10]);
        assert_eq!(&live[1].samples[6..], &[0.0; 10]);
        assert_eq!(edit.generation, 7);
        assert_eq!(edit.bar_markers, [1]);
        assert_eq!(edit.snapshots[0].samples, [0.25; 6]);
        assert_eq!(edit.snapshots[1].samples, [-0.5; 6]);

        controller.set_retain_silence(false);
        controller.request_snapshots(None).recv().unwrap();
        {
            let mut session = lock(&controller.state);
            assert!(!session.retain_silence);
            assert_eq!(session.generation, 7);
            session.capture.as_mut().unwrap().push_native_interleaved_with_markers(
                1_600_000_000, &[0.0; 200], false, &[],
            ).unwrap();
        }
        let after = controller.snapshots(None).unwrap();
        assert!(after[0].frame_range().end < 116);
        assert!(after[0].frame_range().end >= 16);
        assert_eq!(after[0].frame_range(), after[1].frame_range());
        assert_eq!(&after[0].samples[..16], &live[0].samples);
        assert_eq!(&after[1].samples[..16], &live[1].samples);
    }

    #[test]
    fn quiet_retention_retries_a_failed_requested_policy_without_another_ui_change() {
        let mut session = CaptureSession::new();
        session.paused = false;
        session.next_retry = Instant::now() + Duration::from_secs(3600);
        session.requested_retain_silence = true;
        session.mode_error = Some("temporary source-control timeout".into());
        session.next_mode_retry = Instant::now();
        let mut capture =
            SynchronizedCapture::new(10, 1, None, DurationChoice::Seconds20).unwrap();
        capture.push_native_interleaved(0, &[0.25; 4], false).unwrap();
        session.capture = Some(capture);

        session.poll();

        assert!(session.retain_silence);
        assert!(session.requested_retain_silence);
        assert!(session.mode_error.is_none());
        assert!(!session.paused);
        let capture = session.capture.as_ref().unwrap();
        assert!(capture.retain_silence());
        assert_eq!(capture.capture().frame_range(), 0..4);
        assert_eq!(capture.capture().track_snapshot_range(0, 0..4).unwrap().samples, [0.25; 4]);
    }

    #[test]
    fn quiet_retention_is_applied_when_the_source_format_is_created_or_changes() {
        let mut session = CaptureSession::new();
        session.set_retain_silence(true);
        session.formats[0] = Some(Arc::new(Format::sender(10, 2)));
        session.configure_capture();
        assert!(session.capture.as_ref().unwrap().retain_silence());
        assert_eq!(session.shape, Some((10, 2, None)));
        session.formats[0] = Some(Arc::new(Format::sender(20, 1)));
        session.configure_capture();
        assert!(session.capture.as_ref().unwrap().retain_silence());
        assert_eq!(session.shape, Some((20, 1, None)));
    }

    #[test]
    fn paint_metadata_never_waits_for_busy_capture_work() {
        let mut controller = Controller::new().unwrap();
        {
            let mut session = lock(&controller.state);
            session.generation = 7;
            session.revision = 9;
            let mut capture =
                SynchronizedCapture::new(10, 2, None, DurationChoice::Seconds20).unwrap();
            capture
                .capture_mut()
                .push_interleaved(&[0.5, -0.25])
                .unwrap();
            session.capture = Some(capture);
        }
        controller.poll();
        let state = Arc::clone(&controller.state);
        let busy = lock(&state);
        let (started, start) = mpsc::sync_channel(1);
        let (finished, finish) = mpsc::sync_channel(1);
        let paint = thread::spawn(move || {
            started.send(()).unwrap();
            controller.poll();
            assert!(controller.is_paused());
            assert!(!controller.is_recording());
            assert_eq!(controller.generation(), 7);
            assert_eq!(controller.revision(), 9);
            assert_eq!(controller.track_channels(0), Some(2));
            assert!(
                controller
                    .track_meter_readings(0)
                    .unwrap()
                    .peak_dbfs
                    .is_finite()
            );
            let mut minimum = [-123.0; 4];
            let mut maximum = [456.0; 4];
            assert!(
                controller
                    .try_render_waveforms(
                        &[(0, WaveformView::Merged)],
                        None,
                        &mut [(&mut minimum, &mut maximum)],
                    )
                    .is_none()
            );
            assert_eq!(minimum, [-123.0; 4]);
            assert_eq!(maximum, [456.0; 4]);
            finished.send(()).unwrap();
        });
        start.recv().unwrap();
        let returned_while_busy = finish.recv_timeout(Duration::from_millis(100)).is_ok();
        drop(busy);
        paint.join().unwrap();
        assert!(
            returned_while_busy,
            "paint metadata waited for the capture mutex"
        );
    }

    #[test]
    fn snapshot_request_returns_while_busy_and_copies_the_requested_original_frames() {
        let controller = Controller::new().unwrap();
        let state = Arc::clone(&controller.state);
        {
            let mut session = lock(&state);
            let mut capture =
                SynchronizedCapture::new(10, 1, None, DurationChoice::Seconds20).unwrap();
            capture
                .capture_mut()
                .push_interleaved(&[0.1, 0.2, 0.3, 0.4])
                .unwrap();
            session.capture = Some(capture);
        }
        let busy = lock(&state);
        let (posted, request) = mpsc::sync_channel(1);
        let painter = thread::spawn(move || {
            posted
                .send(controller.request_snapshots(Some(1..3)))
                .unwrap();
        });
        let result = request.recv_timeout(Duration::from_millis(100));
        let returned_while_busy = result.is_ok();
        drop(busy);
        painter.join().unwrap();
        assert!(returned_while_busy, "snapshot request waited for capture work");
        let snapshots = result.unwrap().recv().unwrap().unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].frame_range(), 1..3);
        assert_eq!(snapshots[0].samples, [0.2, 0.3]);
    }

    #[test]
    fn edit_snapshot_has_full_atomic_bar_history_and_survives_live_retention_trim() {
        let mut controller = Controller::new().unwrap();
        {
            let mut session = lock(&controller.state);
            session.sources[1] = Some(SourceSettings::default());
            session.shape = Some((10, 1, Some(1)));
            session.generation = 7;
            let mut capture =
                SynchronizedCapture::new(10, 1, Some(1), DurationChoice::Minutes10).unwrap();
            let input: Vec<_> = (0..200).flat_map(|_| [0.25, -0.5]).collect();
            capture
                .push_native_interleaved_with_markers(0, &input, false, &[10, 30, 170, 190])
                .unwrap();
            session.capture = Some(capture);
        }
        let mut minimum = [0.0; 2];
        let mut maximum = [0.0; 2];
        let (_, _, _, visible_bars) = controller.render_waveforms(
            &[(0, WaveformView::Merged)],
            Some(20..40),
            &mut [(&mut minimum, &mut maximum)],
        );
        assert_eq!(visible_bars, [30]);
        let edit = controller.request_edit_snapshot().recv().unwrap().unwrap();
        assert_eq!(edit.generation, 7);
        assert_eq!(edit.bar_markers, [10, 30, 170, 190]);
        assert_eq!(edit.snapshots.len(), 2);
        assert!(edit.snapshots.iter().all(|snapshot| snapshot.frame_range() == (0..200)));

        controller.set_retention(DurationChoice::Seconds10);
        let live = controller.snapshots(None).unwrap();
        assert_eq!(live[0].frame_range(), 100..200);
        assert_eq!(edit.bar_markers, [10, 30, 170, 190]);
        assert_eq!(edit.snapshots[0].frame_range(), 0..200);
        assert_eq!(edit.snapshots[0].samples, vec![0.25; 200]);
        assert_eq!(edit.snapshots[1].samples, vec![-0.5; 200]);
    }

    #[test]
    fn background_retention_and_snapshots_keep_a_and_b_on_the_same_frames() {
        let mut controller = Controller::new().unwrap();
        {
            let mut session = lock(&controller.state);
            session.sources[1] = Some(SourceSettings::default());
            session.shape = Some((10, 1, Some(1)));
            let mut capture =
                SynchronizedCapture::new(10, 1, Some(1), DurationChoice::Minutes5).unwrap();
            let input: Vec<_> = (1..=500)
                .flat_map(|frame| [frame as f32, -(frame as f32)])
                .collect();
            capture
                .push_native_interleaved_with_markers(0, &input, false, &[100, 300, 420])
                .unwrap();
            capture.capture_mut().pause();
            session.capture = Some(capture);
        }
        controller.set_retention(DurationChoice::Seconds30);
        let snapshots = controller.snapshots(None).unwrap();
        assert_eq!(snapshots.len(), 2);
        assert_eq!(snapshots[0].frame_range(), 200..500);
        assert_eq!(snapshots[1].frame_range(), 200..500);
        assert_eq!(snapshots[0].samples[0], 201.0);
        assert_eq!(snapshots[1].samples[0], -201.0);
        let selected = controller.snapshots(Some(298..302)).unwrap();
        assert_eq!(selected[0].frame_range(), 298..302);
        assert_eq!(selected[0].samples, [299.0, 300.0, 301.0, 302.0]);
        assert_eq!(selected[1].samples, [-299.0, -300.0, -301.0, -302.0]);

        let mut low = [0.0; 2];
        let mut high = [0.0; 2];
        let (history, visible, rate, markers) = controller.render_waveforms(
            &[(1, WaveformView::Merged)],
            Some(300..400),
            &mut [(&mut low, &mut high)],
        );
        assert_eq!(history, 200..500);
        assert_eq!(visible, 300..400);
        assert_eq!(rate, 10);
        assert_eq!(markers, [300]);
        assert_eq!(low, [-350.0, -400.0]);
        assert_eq!(high, [-301.0, -351.0]);
    }
}
