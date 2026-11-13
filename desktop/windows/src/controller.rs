use std::io;
use std::ops::Range;
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use backtracking_core::{
    CaptureError, CaptureState, ChannelRoute, DurationChoice, MeterReadings, SenderReceiver,
    Snapshot, SynchronizedCapture, TimedPacket, WaveformView, discover_senders,
};

pub(crate) use crate::loopback::CaptureSource;
use crate::loopback::{Format, Loopback, PACKET_QUEUE_CAPACITY, now_ns};

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
}

type Sources = [Option<SourceSettings>; 2];
type WaveformBuffers<'a> = (&'a mut [f32], &'a mut [f32]);

enum CaptureCommand {
    SetSources(Sources),
    SetRetention(DurationChoice),
    Pause,
    Resume,
    Clear,
    Snapshot {
        selection: Option<Range<u64>>,
        reply: mpsc::SyncSender<Option<Arc<[Snapshot]>>>,
    },
    Shutdown,
}

struct Request {
    command: CaptureCommand,
    complete: Option<mpsc::SyncSender<()>>,
}

#[derive(Default)]
struct SourceStatus {
    formats: [Option<Arc<Format>>; 2],
    statuses: [Option<String>; 2],
    error: Option<String>,
}

pub(crate) struct Controller {
    sources: Sources,
    retention: DurationChoice,
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
            source_status: SourceStatus::default(),
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
        self.source_status = {
            let state = lock(&self.state);
            SourceStatus {
                formats: state.formats.clone(),
                statuses: state.statuses.clone(),
                error: state.error.clone(),
            }
        };
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
        lock(&self.state).is_paused()
    }

    pub(crate) fn is_recording(&self) -> bool {
        lock(&self.state).is_recording()
    }

    pub(crate) fn generation(&self) -> u64 {
        lock(&self.state).generation
    }

    pub(crate) fn snapshots(&self, selection: Option<Range<u64>>) -> Option<Arc<[Snapshot]>> {
        let (reply, result) = mpsc::sync_channel(1);
        self.commands
            .send(Request {
                command: CaptureCommand::Snapshot { selection, reply },
                complete: None,
            })
            .ok()?;
        result.recv().ok().flatten()
    }

    pub(crate) fn track_meter_readings(&self, lane: usize) -> Option<MeterReadings> {
        lock(&self.state)
            .capture
            .as_ref()?
            .capture()
            .track_meter_readings(lane)
    }

    pub(crate) fn track_channels(&self, lane: usize) -> Option<usize> {
        lock(&self.state)
            .capture
            .as_ref()?
            .capture()
            .track_channels(lane)
    }

    pub(crate) fn render_waveforms(
        &self,
        views: &[(usize, WaveformView)],
        zoom: Option<Range<u64>>,
        traces: &mut [WaveformBuffers<'_>],
    ) -> (Range<u64>, Range<u64>, u32, Vec<u64>) {
        let state = lock(&self.state);
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
        lock(&self.state).revision
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
            let _ = result.recv();
        }
        self.poll();
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
    shape: Option<(u32, usize, Option<usize>)>,
    generation: u64,
    revision: u64,
    paused: bool,
    next_retry: Instant,
    error: Option<String>,
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
            shape: None,
            generation: 0,
            revision: 0,
            paused: true,
            next_retry: Instant::now(),
            error: None,
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
                        CaptureCommand::Pause => state.pause(),
                        CaptureCommand::Resume => state.resume(),
                        CaptureCommand::Clear => state.clear(),
                        CaptureCommand::Snapshot { selection, reply } => {
                            state.poll();
                            let _ = reply.send(state.snapshots(selection));
                        }
                        CaptureCommand::Shutdown => break,
                    }
                    if let Some(complete) = complete {
                        let _ = complete.send(());
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
                Ok(stream) => {
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
