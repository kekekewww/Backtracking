use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, TryLockError, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use backtracking_core::{
    CaptureError, CaptureState, ChannelRoute, ChannelRouting, DurationChoice, FrozenTrack,
    FrozenPositionTrack,
    MeterReadings, SenderInfo, SenderReceiver, SynchronizedCapture, TimedPacket, WaveformView,
    discover_senders,
};

use crate::addons::{AddonHandle, AddonPositions, AddonSnapshot, AddonState};
use crate::addons::position_timeline::PositionTimeline;
use crate::preferences::{
    Key, PreferenceWriter, Preferences, SourceGroupChoices, serialize_shown_groups,
    source_preference_key,
};
mod layout;
mod waveform;
#[cfg(test)]
mod waveform_tests;
pub(crate) use layout::{LaneGroup, LaneMeterReadings};
use layout::{combined_channels, lane_groups, lane_meter_readings, machine_history_budget_samples};
pub(crate) use waveform::ChannelWaveform;
use waveform::ChannelWaveforms;

pub(crate) use crate::loopback::{CaptureSource, SENDER_MARK};
use crate::loopback::{
    DataWake, Format, Loopback, PACKET_QUEUE_CAPACITY, QuietModeBarrier, now_ns,
};

const COMMON_RATE: u32 = 48_000;
const RETRY_DELAY: Duration = Duration::from_secs(2);
const SENDER_LAYOUT_POLL: Duration = Duration::from_millis(250);
/// A Sender source is a socket that nothing announces, so it is read on a timer:
/// quickly while its packets keep arriving and slowly while it is silent.
const SENDER_ACTIVE_POLL: Duration = Duration::from_millis(4);
const SENDER_IDLE_POLL: Duration = Duration::from_millis(20);
const SENDER_ACTIVE_WINDOW: Duration = Duration::from_millis(250);
/// New capture data asks the UI for a repaint at most this often, which is the
/// rate its refresh timer used to run at.
const UI_NOTIFY_INTERVAL: Duration = Duration::from_millis(80);

/// Asks the UI thread for one repaint. Safe to call from any thread.
pub(crate) type RepaintRequest = Arc<dyn Fn() + Send + Sync>;

/// What the capture worker shares with the UI besides the session itself.
struct UiLink {
    repaint: OnceLock<RepaintRequest>,
    /// False while nothing displays the waveform or the meters.
    watching: AtomicBool,
}

/// The parts of a session that the UI draws. The worker compares two of these
/// to decide whether the UI has anything new to show.
#[derive(Clone, Debug, Default, PartialEq)]
struct DisplayStamp {
    /// Everything except the waveform and the meters.
    status: u64,
    revision: u64,
    /// Tenths of a decibel, which is as fine as the readout shows.
    meters: [Option<[i32; 3]>; 2],
}

impl DisplayStamp {
    fn of(state: &CaptureSession) -> Self {
        let capture = state.capture.as_ref().map(SynchronizedCapture::capture);
        let mut status = DefaultHasher::new();
        state.generation.hash(&mut status);
        state.is_paused().hash(&mut status);
        state.is_recording().hash(&mut status);
        state.error.hash(&mut status);
        state.mode_error.hash(&mut status);
        state.statuses.hash(&mut status);
        state.position_overflowed.hash(&mut status);
        for (lane, format) in state.formats.iter().enumerate() {
            format.as_ref().map(Arc::as_ptr).hash(&mut status);
            capture
                .and_then(|capture| capture.track_channels(state.capture_lanes[lane]?))
                .hash(&mut status);
        }
        Self {
            status: status.finish(),
            revision: state.revision,
            meters: std::array::from_fn(|lane| {
                capture
                    .and_then(|capture| capture.track_meter_readings(state.capture_lanes[lane]?))
                    .map(|meter| {
                        [meter.peak_dbfs, meter.vu, meter.momentary_lufs]
                            .map(|value| (value * 10.0).round() as i32)
                    })
            }),
        }
    }

    /// Waveform and meter changes only count while something displays them.
    fn differs(&self, shown: &Self, watching: bool) -> bool {
        self.status != shown.status
            || (watching && (self.revision != shown.revision || self.meters != shown.meters))
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Notify {
    Now,
    Later(Duration),
    Idle,
}

/// Limits UI repaint requests to one per `UI_NOTIFY_INTERVAL`. The first change
/// after a quiet spell goes out at once and the last one is never dropped.
#[derive(Default)]
struct RepaintThrottle {
    last: Option<Instant>,
    dirty: bool,
}

impl RepaintThrottle {
    fn update(&mut self, changed: bool, now: Instant) -> Notify {
        self.dirty |= changed;
        if !self.dirty {
            return Notify::Idle;
        }
        match self.last.map(|last| now.saturating_duration_since(last)) {
            Some(since) if since < UI_NOTIFY_INTERVAL => Notify::Later(UI_NOTIFY_INTERVAL - since),
            _ => {
                self.dirty = false;
                self.last = Some(now);
                Notify::Now
            }
        }
    }
}

struct WorkerWait {
    paused: bool,
    sender_connected: bool,
    since_sender_packet: Option<Duration>,
    mode_retry_in: Option<Duration>,
    source_retry_in: Option<Duration>,
}

/// How long the capture worker may block before it has work that no command
/// or capture-thread wake announces. `None` blocks until one arrives.
fn worker_wait(state: &WorkerWait) -> Option<Duration> {
    if state.paused {
        return None;
    }
    let sender = state.sender_connected.then(|| {
        if state
            .since_sender_packet
            .is_some_and(|since| since < SENDER_ACTIVE_WINDOW)
        {
            SENDER_ACTIVE_POLL
        } else {
            SENDER_IDLE_POLL
        }
    });
    [sender, state.mode_retry_in, state.source_retry_in]
        .into_iter()
        .flatten()
        .min()
}

/// Wakes the capture worker when dropped, including during an unwind.
struct WakeOnDrop(DataWake);

impl Drop for WakeOnDrop {
    fn drop(&mut self) {
        (self.0)();
    }
}

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
    fn open(source: CaptureSource, wake: DataWake, all_channels: bool) -> Result<Self, String> {
        match source {
            CaptureSource::Sender { id, .. } => {
                let receiver = SenderReceiver::connect(&id).map_err(|error| error.to_string())?;
                let format = Arc::new(
                    Format::sender(receiver.info.sample_rate, receiver.info.channels)
                        .with_sender_labels(receiver.labels()),
                );
                Ok(Self::Sender {
                    receiver: Some(receiver),
                    format,
                    id,
                    error: None,
                })
            }
            _ => Loopback::start(source, wake, all_channels).map(Self::Wasapi),
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
            Self::Sender {
                receiver, error, ..
            } => match receiver.as_mut()?.packet(samples, bars) {
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
                receiver,
                format,
                id,
                ..
            } => {
                let next = SenderReceiver::connect(id).map_err(|error| error.to_string())?;
                if next.info.sample_rate != format.sample_rate
                    || next.info.channels != format.channels
                    || (format.channels > 2 && next.labels() != format.sender_labels())
                {
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

fn requests_all_channels(sources: &Sources, source: &CaptureSource) -> bool {
    sources
        .iter()
        .flatten()
        .any(|lane| lane.source.same_source(source) && lane.route == ChannelRoute::AllChannels)
}

/// The entries every source list starts with: system audio and the default microphone.
const FIXED_SOURCES: usize = 2;

/// Lists plug-in senders right after the fixed entries. Someone switched each one on, and a
/// long list of devices and applications would otherwise push them out of the visible rows.
fn with_senders(mut sources: Vec<CaptureSource>, senders: Vec<SenderInfo>) -> Vec<CaptureSource> {
    let at = sources.len().min(FIXED_SOURCES);
    sources.splice(
        at..at,
        senders.into_iter().map(|sender| CaptureSource::Sender {
            id: sender.id,
            name: format!("{SENDER_MARK}{}", sender.name),
        }),
    );
    sources
}

fn sources_with_addons(
    sources: Vec<CaptureSource>,
    senders: Vec<SenderInfo>,
    snapshot: &AddonSnapshot,
) -> Vec<CaptureSource> {
    let owned: std::collections::HashSet<_> = snapshot
        .addons
        .iter()
        .flat_map(|addon| addon.sources.iter().map(|source| source.sender.as_str()))
        .collect();
    let mut sources = with_senders(
        sources,
        senders
            .into_iter()
            .filter(|sender| !owned.contains(sender.id.as_str()))
            .collect(),
    );
    let entries = snapshot
        .addons
        .iter()
        .filter(|addon| addon.state == AddonState::Running)
        .flat_map(|addon| {
            addon.sources.iter().map(|source| CaptureSource::Addon {
                addon_id: addon.id.clone(),
                source_id: source.id.clone(),
                name: source.name.clone(),
            })
        });
    let at = sources.len().min(FIXED_SOURCES);
    sources.splice(at..at, entries);
    sources
}

fn is_sender(source: &CaptureSource) -> bool {
    matches!(
        source,
        CaptureSource::Sender { .. } | CaptureSource::Addon { .. }
    )
}

fn effective_route(format: &Format, route: ChannelRoute) -> ChannelRoute {
    if route == ChannelRoute::AllChannels && format.channels <= 2 {
        ChannelRoute::Merged
    } else {
        route
    }
}

fn capture_channels(source: &CaptureSource, format: &Format, route: ChannelRoute) -> Option<usize> {
    if is_sender(source) && format.channels > 2 {
        ChannelRouting::Native.output_channels(format.channels, None)
    } else {
        format.route_channels(effective_route(format, route))
    }
}

fn route_samples(
    source: &CaptureSource,
    format: &Format,
    route: ChannelRoute,
    samples: &[f32],
    output: &mut Vec<f32>,
) {
    if is_sender(source) && format.channels > 2 {
        let _ = ChannelRouting::Native.route_interleaved(samples, format.channels, None, output);
    } else {
        format.route(effective_route(format, route), samples, output);
    }
}
type WaveformBuffers<'a> = (&'a mut [f32], &'a mut [f32]);

pub(crate) struct EditSnapshot {
    /// The lanes of one frozen view. They share the capture's pages.
    pub(crate) snapshots: Arc<[FrozenTrack]>,
    /// Position history by logical lane, shared with this view until live writes diverge.
    #[allow(dead_code)] // Export and moving-source rendering consume this in the next wave.
    pub(crate) positions: [FrozenPositionTrack; 2],
    pub(crate) bar_markers: Vec<u64>,
    pub(crate) generation: u64,
    /// Group metadata is frozen with the audio, before a later format change.
    pub(crate) groups: [Vec<LaneGroup>; 2],
    pub(crate) shown_groups: [Option<String>; 2],
    /// Physical snapshot index to logical lane (A = 0, B = 1).
    pub(crate) lanes: Vec<usize>,
}

enum CaptureCommand {
    SetSources(Sources),
    SetRetention(DurationChoice),
    SetRetainSilence(bool),
    SetShownGroups(SourceGroupChoices),
    Pause,
    Resume,
    Clear,
    Snapshot {
        selection: Option<Range<u64>>,
        reply: mpsc::SyncSender<Option<Arc<[FrozenTrack]>>>,
    },
    EditSnapshot {
        reply: mpsc::SyncSender<Option<EditSnapshot>>,
    },
    /// A capture thread queued packets or an error, or a source finished opening.
    Wake,
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
    groups: [Vec<LaneGroup>; 2],
    shown_groups: [Option<String>; 2],
    effective_retention_seconds: Option<f64>,
    history_budget_samples: usize,
    lanes: Vec<usize>,
}

impl SourceStatus {
    fn from_session(state: &CaptureSession) -> Self {
        let capture = state.capture.as_ref().map(SynchronizedCapture::capture);
        Self {
            formats: state.formats.clone(),
            statuses: std::array::from_fn(|lane| {
                state.statuses[lane].clone().or_else(|| {
                state.position_overflowed[lane].then(|| {
                    "Position history was trimmed to its memory limit. / 位置歷史已依記憶體上限刪減。".into()
                })
            })
            }),
            error: state.error.clone().or_else(|| state.mode_error.clone()),
            paused: state.is_paused(),
            recording: state.is_recording(),
            generation: state.generation,
            revision: state.revision,
            channels: std::array::from_fn(|lane| {
                capture.and_then(|capture| capture.track_channels(state.capture_lanes[lane]?))
            }),
            meters: std::array::from_fn(|lane| {
                capture.and_then(|capture| capture.track_meter_readings(state.capture_lanes[lane]?))
            }),
            groups: state.groups.clone(),
            shown_groups: state.current_shown_groups(),
            effective_retention_seconds: capture
                .map(|capture| capture.effective_retention_seconds()),
            history_budget_samples: state.history_budget_samples,
            lanes: state.active_lanes(),
        }
    }
}

#[derive(Default)]
struct ActivityRead {
    generation: u64,
    mask: Option<(u64, u64)>, // capture revision and active-channel bits
    pending: Option<mpsc::Receiver<Option<ActivityResult>>>,
    #[cfg(test)]
    worker_time: Duration,
}
struct ActivityResult {
    revision: u64,
    mask: u64,
    #[cfg(test)]
    worker_time: Duration,
}

pub(crate) struct Controller {
    sources: Sources,
    retention: DurationChoice,
    retain_silence: bool,
    source_status: SourceStatus,
    state: Arc<Mutex<CaptureSession>>,
    ui: Arc<UiLink>,
    commands: mpsc::Sender<Request>,
    worker: Option<JoinHandle<()>>,
    addons: Option<AddonHandle>,
    shown_groups: SourceGroupChoices,
    group_preferences: Option<PreferenceWriter>,
    channel_waveforms: Mutex<ChannelWaveforms>,
    activity: Mutex<[ActivityRead; 2]>,
    #[cfg(test)]
    channel_waveform_lock_ns: std::sync::atomic::AtomicU64,
}

impl Controller {
    pub(crate) fn new() -> io::Result<Self> {
        Self::create(None)
    }

    pub(crate) fn with_addons(addons: AddonHandle) -> io::Result<Self> {
        Self::create(Some(addons))
    }

    fn create(addons: Option<AddonHandle>) -> io::Result<Self> {
        let (commands, requests) = mpsc::channel();
        // Many packets can arrive before the worker runs. One queued wake covers them all.
        let wake_pending = Arc::new(AtomicBool::new(false));
        let wake: DataWake = {
            let commands = commands.clone();
            let pending = Arc::clone(&wake_pending);
            Arc::new(move || {
                if !pending.swap(true, Ordering::AcqRel) {
                    let _ = commands.send(Request {
                        command: CaptureCommand::Wake,
                        complete: None,
                    });
                }
            })
        };
        if let Some(addons) = &addons {
            addons.subscribe(Arc::clone(&wake));
        }
        let mut session = CaptureSession::with_wake(wake);
        session.addons = addons.clone();
        let history_budget_samples = session.history_budget_samples;
        let state = Arc::new(Mutex::new(session));
        let ui = Arc::new(UiLink {
            repaint: OnceLock::new(),
            watching: AtomicBool::new(true),
        });
        let worker_state = Arc::clone(&state);
        let worker_ui = Arc::clone(&ui);
        let channel_waveforms = Mutex::new(ChannelWaveforms::new(Arc::clone(&ui))?);
        let worker = thread::Builder::new()
            .name("backtracking-capture".into())
            .spawn(move || {
                CaptureSession::run(&worker_state, requests, &wake_pending, &worker_ui)
            })?;
        Ok(Self {
            sources: [Some(SourceSettings::default()), None],
            retention: DurationChoice::Seconds20,
            retain_silence: false,
            source_status: SourceStatus {
                paused: true,
                history_budget_samples,
                ..SourceStatus::default()
            },
            state,
            ui,
            commands,
            worker: Some(worker),
            addons,
            shown_groups: SourceGroupChoices::new(),
            group_preferences: None,
            channel_waveforms,
            activity: Default::default(),
            #[cfg(test)]
            channel_waveform_lock_ns: std::sync::atomic::AtomicU64::new(0),
        })
    }

    /// Installs the way the capture worker asks for a repaint when what the UI
    /// draws has changed. Until then the UI only sees the changes it polls for.
    pub(crate) fn set_repaint(&self, repaint: RepaintRequest) {
        if let Some(addons) = &self.addons {
            addons.subscribe(Arc::clone(&repaint));
        }
        let _ = self.ui.repaint.set(repaint);
    }

    /// Tells the worker whether the waveform and meters are on screen. While
    /// they are not, only status changes ask for a repaint.
    pub(crate) fn set_watching(&self, watching: bool) {
        self.ui.watching.store(watching, Ordering::Relaxed);
    }

    pub(crate) fn sources(&self) -> &Sources {
        &self.sources
    }

    pub(crate) fn set_sources(&mut self, mut sources: Sources) {
        if sources[0].is_none() {
            sources[0] = sources[1]
                .take()
                .or_else(|| Some(SourceSettings::default()));
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
        with_senders(Loopback::available_sources(), discover_senders())
    }

    pub(crate) fn available_sources_with_addons(snapshot: &AddonSnapshot) -> Vec<CaptureSource> {
        sources_with_addons(Loopback::available_sources(), discover_senders(), snapshot)
    }

    pub(crate) fn addons(&self) -> Option<&AddonHandle> {
        self.addons.as_ref()
    }

    /// Enables persistent group choices using the same injected preferences as the recorder.
    pub(crate) fn set_group_preferences(&mut self, preferences: Preferences) -> io::Result<()> {
        let choices = preferences.load_shown_groups();
        self.group_preferences = Some(PreferenceWriter::new(preferences)?);
        self.set_shown_groups(choices);
        Ok(())
    }

    pub(crate) fn set_shown_groups(&mut self, choices: SourceGroupChoices) {
        self.shown_groups = choices.clone();
        self.request(CaptureCommand::SetShownGroups(choices));
    }

    pub(crate) fn shown_groups(&self) -> &SourceGroupChoices {
        &self.shown_groups
    }

    pub(crate) fn lane_groups(&self, lane: usize) -> &[LaneGroup] {
        self.source_status
            .groups
            .get(lane)
            .map_or(&[], Vec::as_slice)
    }

    pub(crate) fn shown_group(&self, lane: usize) -> Option<&LaneGroup> {
        let id = self.source_status.shown_groups.get(lane)?.as_ref()?;
        self.lane_groups(lane).iter().find(|group| group.id == *id)
    }

    pub(crate) fn select_group(&mut self, lane: usize, id: &str) -> Result<(), String> {
        if !self.lane_groups(lane).iter().any(|group| group.id == id) {
            return Err("This channel group is not available".into());
        }
        let source = &self
            .sources
            .get(lane)
            .and_then(Option::as_ref)
            .ok_or("This lane has no source")?
            .source;
        self.shown_groups
            .insert(source_preference_key(source), id.into());
        if let Some(writer) = &self.group_preferences {
            writer
                .save(
                    Key::ShownGroups,
                    &serialize_shown_groups(&self.shown_groups),
                )
                .map_err(|error| error.to_string())?;
        }
        self.request(CaptureCommand::SetShownGroups(self.shown_groups.clone()));
        Ok(())
    }

    pub(crate) fn group_preference_error(&self) -> Option<io::Error> {
        self.group_preferences
            .as_ref()
            .and_then(PreferenceWriter::poll_error)
    }

    /// Returns the newest completed mask for the displayed generation. Freezing
    /// pages and building cold channel peaks both happen on the activity worker.
    #[allow(dead_code)] // Used by the wide-lane row filter in the next UI wave.
    pub(crate) fn try_active_channels(&self, lane: usize) -> Option<u64> {
        if self.track_channels(lane)? <= 2 {
            return Some(0);
        }
        let generation = self.source_status.generation;
        let mut reads = self.activity.try_lock().ok()?;
        let read = reads.get_mut(lane)?;
        if read.generation != generation {
            *read = ActivityRead {
                generation,
                ..Default::default()
            };
        }
        if let Some(pending) = &read.pending {
            match pending.try_recv() {
                Ok(Some(result)) => {
                    read.mask = Some((result.revision, result.mask));
                    #[cfg(test)]
                    {
                        read.worker_time = result.worker_time;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => return read.mask.map(|(_, mask)| mask),
                _ => {}
            }
            read.pending = None;
        }
        let last = read.mask.map(|(_, mask)| mask);
        if read
            .mask
            .is_some_and(|(rev, _)| rev >= self.source_status.revision)
        {
            return last;
        }
        if let Ok(worker) = self.channel_waveforms.try_lock() {
            let (send, result) = mpsc::channel();
            let state = Arc::downgrade(&self.state);
            if worker.try_queue(move || {
                let result = (|| {
                    let state = state.upgrade()?;
                    let state = lock(&state);
                    if state.generation != generation {
                        return None;
                    }
                    let revision = state.revision;
                    let physical = state.capture_lanes[lane]?;
                    let track = state.capture.as_ref()?.capture().freeze().track(physical)?;
                    drop(state);
                    #[cfg(test)]
                    let start = Instant::now();
                    let mask = track.active_channels();
                    Some(ActivityResult {
                        revision,
                        mask,
                        #[cfg(test)]
                        worker_time: start.elapsed(),
                    })
                })();
                let _ = send.send(result);
            }) {
                read.pending = Some(result);
            }
        }
        last
    }

    pub(crate) fn route_available(&self, lane: usize, route: ChannelRoute) -> bool {
        if self
            .sources
            .get(lane)
            .and_then(Option::as_ref)
            .is_some_and(|s| is_sender(&s.source))
            && self
                .track_channels(lane)
                .is_some_and(|channels| channels > 2)
        {
            return route == ChannelRoute::Merged;
        }
        if let Some(format) = self
            .source_status
            .formats
            .get(lane)
            .and_then(Option::as_ref)
        {
            if route == ChannelRoute::AllChannels {
                return format.offers_all_channels();
            }
            return format.route_channels(route).is_some();
        }
        // Until a source opens, avoid rejecting a saved route that the device may support.
        !matches!(route, ChannelRoute::Center | ChannelRoute::AllChannels)
    }

    pub(crate) fn source_status(&self, lane: usize) -> Option<&str> {
        self.source_status
            .statuses
            .get(lane)
            .and_then(Option::as_deref)
    }

    /// Returns false when the capture worker was busy and the status is unchanged.
    pub(crate) fn poll(&mut self) -> bool {
        // Audio work can include a cold allocation, page release, or a snapshot
        // copy. A paint must retain the last complete metadata instead of
        // waiting for that work on the UI thread.
        let Some(state) = try_lock(&self.state) else {
            return false;
        };
        self.source_status = SourceStatus::from_session(&state);
        true
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
    pub(crate) fn snapshots(&self, selection: Option<Range<u64>>) -> Option<Arc<[FrozenTrack]>> {
        self.request_snapshots(selection).recv().ok().flatten()
    }

    /// Asks the capture worker to pin the audio as it is now. Receiving its result
    /// is optional and must be polled, rather than waited for, from the UI thread.
    pub(crate) fn request_snapshots(
        &self,
        selection: Option<Range<u64>>,
    ) -> mpsc::Receiver<Option<Arc<[FrozenTrack]>>> {
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

    pub(crate) fn lane_meter_readings(&self, lane: usize) -> Option<LaneMeterReadings> {
        Some(lane_meter_readings(
            self.track_meter_readings(lane)?,
            self.track_channels(lane)?,
        ))
    }

    pub(crate) fn effective_retention_seconds(&self) -> Option<f64> {
        self.source_status.effective_retention_seconds
    }

    pub(crate) fn history_budget_samples(&self) -> usize {
        self.source_status.history_budget_samples
    }

    /// `try_render_rows` for native channels only, as the tests ask for them.
    #[cfg(test)]
    pub(crate) fn try_render_channel_waveforms(
        &self,
        channels: &[(usize, usize)],
        views: &[(usize, WaveformView)],
        zoom: Option<Range<u64>>,
        traces: &mut [WaveformBuffers<'_>],
        view_traces: &mut [WaveformBuffers<'_>],
    ) -> Option<waveform::RenderedRange> {
        let rows: Vec<_> = channels
            .iter()
            .map(|&(lane, index)| (lane, ChannelWaveform::Native(index)))
            .collect();
        self.try_render_rows(&rows, views, zoom, traces, view_traces)
    }

    /// Pins the visible audio, then renders requested rows off the capture and UI
    /// threads. Pending work leaves every bin untouched. The last field is the
    /// rendered revision, which can precede the newest capture revision.
    pub(crate) fn try_render_rows(
        &self,
        channels: &[(usize, ChannelWaveform)],
        views: &[(usize, WaveformView)],
        zoom: Option<Range<u64>>,
        traces: &mut [WaveformBuffers<'_>],
        view_traces: &mut [WaveformBuffers<'_>],
    ) -> Option<waveform::RenderedRange> {
        let state = try_lock(&self.state)?;
        #[cfg(test)]
        let locked_at = Instant::now();
        let rendered = self
            .channel_waveforms
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .render(&state, channels, views, zoom, traces, view_traces);
        #[cfg(test)]
        self.channel_waveform_lock_ns
            .store(locked_at.elapsed().as_nanos() as u64, Ordering::Relaxed);
        rendered
    }

    pub(crate) fn track_channels(&self, lane: usize) -> Option<usize> {
        self.source_status.channels.get(lane).copied().flatten()
    }

    pub(crate) fn active_lanes(&self) -> &[usize] {
        &self.source_status.lanes
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
                backtracking_ui::fit_frame_range(
                    &history,
                    zoom.start,
                    zoom.end.saturating_sub(zoom.start),
                )
            },
        );
        for (&(lane, view), (minimum, maximum)) in views.iter().zip(traces) {
            match state.capture_lanes.get(lane).copied().flatten() {
                Some(lane) => {
                    capture.track_waveform_range(lane, view, visible.clone(), minimum, maximum);
                }
                None => {
                    minimum.fill(0.0);
                    maximum.fill(0.0);
                }
            }
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
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
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
    /// Wakes the worker that owns this session. Every capture thread gets a copy.
    wake: DataWake,
    sender_packet_at: Option<Instant>,
    addons: Option<AddonHandle>,
    addon_snapshot: Option<Arc<AddonSnapshot>>,
    positions: [PositionTimeline; 2],
    position_cursor: u64,
    position_overflowed: [bool; 2],
    groups: [Vec<LaneGroup>; 2],
    shown_groups: SourceGroupChoices,
    history_budget_samples: usize,
    next_layout_check: Instant,
    layout_blocked: [bool; 2],
    capture_lanes: [Option<usize>; 2],
}

impl CaptureSession {
    #[cfg(test)]
    fn new() -> Self {
        Self::with_wake(Arc::new(|| {}))
    }

    fn with_wake(wake: DataWake) -> Self {
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
            wake,
            sender_packet_at: None,
            addons: None,
            addon_snapshot: None,
            positions: std::array::from_fn(|_| PositionTimeline::default()),
            position_cursor: 0,
            position_overflowed: [false; 2],
            groups: std::array::from_fn(|_| Vec::new()),
            shown_groups: SourceGroupChoices::new(),
            history_budget_samples: machine_history_budget_samples(),
            next_layout_check: Instant::now(),
            layout_blocked: [false; 2],
            capture_lanes: [Some(0), Some(1)],
        }
    }

    fn run(
        session: &Mutex<Self>,
        requests: mpsc::Receiver<Request>,
        wake_pending: &AtomicBool,
        ui: &UiLink,
    ) {
        let mut wait = None;
        let mut shown = DisplayStamp::default();
        let mut throttle = RepaintThrottle::default();
        loop {
            let request = match wait {
                Some(wait) => requests.recv_timeout(wait),
                None => requests
                    .recv()
                    .map_err(|_| mpsc::RecvTimeoutError::Disconnected),
            };
            let mut state = lock(session);
            match request {
                Ok(Request { command, complete }) => {
                    match command {
                        CaptureCommand::SetSources(sources) => state.set_sources(sources),
                        CaptureCommand::SetRetention(retention) => state.set_retention(retention),
                        CaptureCommand::SetRetainSilence(retain_silence) => {
                            state.set_retain_silence(retain_silence);
                        }
                        CaptureCommand::SetShownGroups(choices) => {
                            state.shown_groups = choices;
                            state.revision = state.revision.wrapping_add(1);
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
                        CaptureCommand::Wake => {
                            // Cleared first: a packet queued from here on sends a new wake.
                            wake_pending.store(false, Ordering::Release);
                            state.poll();
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
            let now = Instant::now();
            let stamp = DisplayStamp::of(&state);
            wait = state.next_wait(now);
            drop(state);
            let changed = stamp.differs(&shown, ui.watching.load(Ordering::Relaxed));
            match throttle.update(changed, now) {
                Notify::Now => {
                    shown = stamp;
                    if let Some(repaint) = ui.repaint.get() {
                        repaint();
                    }
                }
                // Wake up to deliver the newest state even if nothing else happens.
                Notify::Later(delay) => wait = Some(wait.map_or(delay, |wait| wait.min(delay))),
                Notify::Idle => {}
            }
        }
    }

    fn source_missing(&self) -> bool {
        self.sources.iter().flatten().any(|lane| {
            self.resolved_source(&lane.source).is_ok()
                && !self
                    .inputs
                    .iter()
                    .any(|input| input.source.same_source(&lane.source))
                && !self
                    .pending
                    .iter()
                    .any(|pending| pending.source.same_source(&lane.source))
        })
    }

    fn next_wait(&self, now: Instant) -> Option<Duration> {
        let wait = worker_wait(&WorkerWait {
            paused: self.paused,
            sender_connected: self.inputs.iter().any(|input| {
                matches!(
                    &input.stream,
                    Stream::Sender {
                        receiver: Some(_),
                        ..
                    }
                )
            }),
            since_sender_packet: self
                .sender_packet_at
                .map(|at| now.saturating_duration_since(at)),
            mode_retry_in: (self.requested_retain_silence != self.retain_silence)
                .then(|| self.next_mode_retry.saturating_duration_since(now)),
            source_retry_in: self
                .source_missing()
                .then(|| self.next_retry.saturating_duration_since(now)),
        });
        if !self.paused
            && self
                .sources
                .iter()
                .flatten()
                .any(|lane| is_sender(&lane.source))
        {
            let layout = self.next_layout_check.saturating_duration_since(now);
            Some(wait.map_or(layout, |wait| wait.min(layout)))
        } else {
            wait
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
        let previous_sources = std::mem::replace(&mut self.sources, sources);
        if !settings_changed {
            return;
        }

        let keep_input = |source: &CaptureSource| {
            self.sources
                .iter()
                .flatten()
                .any(|lane| lane.source.same_source(source))
                && (!matches!(source, CaptureSource::Application { .. })
                    || requests_all_channels(&previous_sources, source)
                        == requests_all_channels(&self.sources, source))
        };
        self.inputs.retain(|input| keep_input(&input.source));
        self.pending.retain(|pending| keep_input(&pending.source));
        self.discard_before_ns = now_ns();
        for input in &mut self.inputs {
            input
                .stream
                .discard_pending(&mut self.native, &mut self.bars);
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
            missing_capture_channel(&settings.source, format, settings.route)
        });
        self.error = None;
        self.layout_blocked = [false; 2];
        self.groups = std::array::from_fn(|_| Vec::new());
        self.reset_capture();
        self.configure_capture();
        if !self.paused {
            self.request_missing();
        }
    }

    fn poll(&mut self) {
        self.refresh_addon_state();
        self.refresh_positions();
        if self.paused {
            return;
        }
        self.check_sender_layouts();
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

    fn resolved_source(&self, source: &CaptureSource) -> Result<CaptureSource, String> {
        let CaptureSource::Addon {
            addon_id,
            source_id,
            ..
        } = source
        else {
            return Ok(source.clone());
        };
        let addon = self
            .addon_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.addon(addon_id))
            .ok_or_else(|| "Add-on is not installed".to_string())?;
        let reason = match &addon.state {
            AddonState::Running => None,
            AddonState::Starting => Some("Add-on is starting".into()),
            AddonState::Stopping => Some("Add-on is stopping".into()),
            AddonState::Stopped => Some("Add-on is stopped".into()),
            AddonState::NotResponding => Some("Add-on is not responding".into()),
            AddonState::Failed(reason) | AddonState::BrokenPackage(reason) => {
                Some(format!("Add-on unavailable: {reason}"))
            }
        };
        if let Some(reason) = reason {
            return Err(reason);
        }
        let source = addon
            .sources
            .iter()
            .find(|source| source.id == *source_id)
            .ok_or_else(|| "Add-on source is not available".to_string())?;
        Ok(CaptureSource::Sender {
            id: source.sender.clone(),
            name: source.name.clone(),
        })
    }

    fn addon_source(
        &self,
        source: &CaptureSource,
    ) -> Option<&backtracking_addon::protocol::Source> {
        let CaptureSource::Addon {
            addon_id,
            source_id,
            ..
        } = source
        else {
            return None;
        };
        self.addon_snapshot.as_ref()?.source(addon_id, source_id)
    }

    fn refresh_addon_state(&mut self) {
        // This worker may wait for the short Arc swap. A UI frame must use try_snapshot instead.
        let Some(next) = self.addons.as_ref().map(AddonHandle::snapshot) else {
            return;
        };
        self.apply_addon_snapshot(next);
    }

    fn refresh_positions(&mut self) {
        let Some(addons) = &self.addons else {
            return;
        };
        let messages = addons.positions_since(&mut self.position_cursor);
        for message in messages {
            self.receive_positions(&message);
        }
    }

    fn receive_positions(&mut self, message: &AddonPositions) {
        if message.time < self.discard_before_ns {
            return;
        }
        for lane in 0..2 {
            let Some(settings) = &self.sources[lane] else {
                continue;
            };
            let CaptureSource::Addon {
                addon_id,
                source_id,
                ..
            } = &settings.source
            else {
                continue;
            };
            if *addon_id != message.addon_id || *source_id != message.source_id {
                continue;
            }
            let Some(source) = self.addon_source(&settings.source) else {
                continue;
            };
            let channels = message
                .channels
                .iter()
                .filter_map(|channel| {
                    let mut channel = *channel;
                    channel.channel =
                        positioned_lane_channel(source, settings.route, channel.channel)?;
                    Some(channel)
                })
                .collect::<Vec<_>>();
            if !channels.is_empty() {
                let clock = self.capture.as_ref().zip(self.capture_lanes[lane]);
                if self.paused
                    && !clock.is_some_and(|(capture, physical)| {
                        matches!(
                            capture.retained_frame_at(physical, message.time),
                            std::task::Poll::Ready(Some(_))
                        )
                    })
                {
                    continue;
                }
                self.positions[lane].receive(message.time, channels);
                if let Some((capture, physical)) = clock {
                    self.positions[lane].resolve_capture(capture, physical);
                }
            }
        }
    }

    fn apply_addon_snapshot(&mut self, next: Arc<AddonSnapshot>) {
        if self
            .addon_snapshot
            .as_ref()
            .is_some_and(|old| old.revision == next.revision)
        {
            return;
        }
        let previous = self.addon_snapshot.replace(next);
        let selected: Vec<_> = self
            .sources
            .iter()
            .flatten()
            .map(|lane| lane.source.clone())
            .collect();
        let mut reconnect = false;
        for source in selected {
            let CaptureSource::Addon {
                addon_id,
                source_id,
                ..
            } = &source
            else {
                continue;
            };
            let old = previous
                .as_ref()
                .and_then(|snapshot| snapshot.source(addon_id, source_id));
            let new = self.addon_source(&source);
            // Only a reopened stream can establish that its audio layout changed.
            let input_changed = match (old, new) {
                (Some(old), Some(new)) => old.sender != new.sender || old.format != new.format,
                (None, None) => false,
                _ => true,
            };
            let groups_changed = match (old, new) {
                (Some(old), Some(new)) => old.groups != new.groups,
                (None, Some(_)) => true,
                _ => false,
            };
            if input_changed {
                self.inputs
                    .retain(|input| !input.source.same_source(&source));
                self.pending
                    .retain(|pending| !pending.source.same_source(&source));
                reconnect = true;
            }
            if groups_changed {
                for lane in 0..2 {
                    if self.sources[lane]
                        .as_ref()
                        .is_some_and(|settings| settings.source.same_source(&source))
                        && self.formats[lane].is_some()
                    {
                        let allowed = self.sources[lane]
                            .as_ref()
                            .and_then(|settings| {
                                let source = self.addon_source(&settings.source)?;
                                Some(
                                    (0..64)
                                        .filter_map(|channel| {
                                            positioned_lane_channel(source, settings.route, channel)
                                                .map(|channel| channel as usize)
                                        })
                                        .collect::<Vec<_>>(),
                                )
                            })
                            .unwrap_or_default();
                        self.positions[lane].retain_channels(&allowed);
                        if !allowed.is_empty()
                            && let Some(physical) = self.capture_lanes[lane]
                            && let Some(capture) = &mut self.capture
                            && let Err(error) = capture.enable_frame_clock(physical)
                        {
                            self.error = Some(error.to_string());
                        }
                        match self.groups_for_lane(lane) {
                            Ok(groups) => {
                                self.groups[lane] = groups;
                                if self.layout_blocked[lane] {
                                    self.statuses[lane] = None;
                                }
                                self.layout_blocked[lane] = false;
                            }
                            Err(reason) => {
                                self.statuses[lane] = Some(reason);
                                self.layout_blocked[lane] = true;
                            }
                        }
                    }
                }
                self.revision = self.revision.wrapping_add(1);
            }
            if let Err(reason) = self.resolved_source(&source) {
                self.set_source_status(&source, reason);
            } else if input_changed {
                self.set_source_status(&source, "Waiting for add-on audio".into());
            }
        }
        if reconnect {
            self.next_retry = Instant::now();
            self.next_layout_check = Instant::now();
        }
    }

    fn check_sender_layouts(&mut self) {
        let now = Instant::now();
        if now < self.next_layout_check {
            return;
        }
        self.next_layout_check = now + SENDER_LAYOUT_POLL;
        if !self.inputs.iter().any(|input| is_sender(&input.source)) {
            return;
        }
        let registrations = discover_senders();
        let mut changed = Vec::new();
        for input in &self.inputs {
            let Stream::Sender { id, format, .. } = &input.stream else {
                continue;
            };
            match registrations.iter().find(|sender| sender.id == *id) {
                Some(sender)
                    if sender.channels == format.channels
                        && sender.sample_rate == format.sample_rate => {}
                Some(_) => changed.push((input.source.clone(), "Waiting for Sender audio layout")),
                None => changed.push((input.source.clone(), "Sender audio is unavailable")),
            }
        }
        if changed.is_empty() {
            return;
        }
        self.inputs.retain(|input| {
            !changed
                .iter()
                .any(|(source, _)| input.source.same_source(source))
        });
        for (source, status) in &changed {
            for (lane, settings) in self.sources.iter().enumerate() {
                if settings
                    .as_ref()
                    .is_some_and(|settings| settings.source.same_source(source))
                {
                    self.statuses[lane] = Some((*status).into());
                    self.layout_blocked[lane] = false;
                }
            }
        }
        self.next_retry = now;
    }

    fn active_lanes(&self) -> Vec<usize> {
        let Some(capture) = self.capture.as_ref() else {
            return Vec::new();
        };
        let mut lanes: Vec<_> = self
            .capture_lanes
            .iter()
            .enumerate()
            .filter_map(|(logical, physical)| {
                let physical = (*physical)?;
                capture
                    .capture()
                    .track_channels(physical)
                    .map(|_| (physical, logical))
            })
            .collect();
        lanes.sort_unstable();
        lanes.into_iter().map(|(_, logical)| logical).collect()
    }

    fn current_shown_groups(&self) -> [Option<String>; 2] {
        std::array::from_fn(|lane| {
            let source = &self.sources[lane].as_ref()?.source;
            let saved = self.shown_groups.get(&source_preference_key(source));
            self.groups[lane]
                .iter()
                .find(|group| Some(&group.id) == saved)
                .or_else(|| self.groups[lane].first())
                .map(|group| group.id.clone())
        })
    }

    fn groups_for_lane(&self, lane: usize) -> Result<Vec<LaneGroup>, String> {
        let settings = self.sources[lane]
            .as_ref()
            .ok_or("This lane has no source")?;
        let format = self.formats[lane]
            .as_ref()
            .ok_or("Waiting for source audio layout")?;
        let channels = capture_channels(&settings.source, format, settings.route)
            .ok_or("The selected channel is unavailable")?;
        if matches!(settings.source, CaptureSource::Addon { .. })
            && self.addon_source(&settings.source).is_none()
            && !self.groups[lane].is_empty()
        {
            return Ok(self.groups[lane].clone());
        }
        let mut native_groups = lane_groups(self.addon_source(&settings.source), format.channels)?;
        if channels == format.channels {
            if channels > 2
                && !matches!(settings.source, CaptureSource::Addon { .. })
                && let Some(labels) = format.speaker_labels()
            {
                native_groups[0].labels = labels;
            }
            return Ok(native_groups);
        }
        // Mono/stereo routing keeps its existing behavior; group coordinates describe its output.
        let mut groups = lane_groups(None, channels)?;
        if channels == 1 {
            groups[0].labels[0] = settings.route.label().into();
        }
        Ok(groups)
    }

    fn request_missing(&mut self) {
        self.refresh_addon_state();
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
            let selected = match self.resolved_source(&source) {
                Ok(source) => source,
                Err(reason) => {
                    self.set_source_status(&source, reason);
                    continue;
                }
            };
            let (sender, result) = mpsc::sync_channel(1);
            let all_channels = requests_all_channels(&self.sources, &source);
            let wake = Arc::clone(&self.wake);
            match thread::Builder::new()
                .name("backtracking-source-open".into())
                .spawn(move || {
                    // Declared first so that it runs last: by then the result is
                    // queued or, after a panic, its sender is gone.
                    let _wake = WakeOnDrop(Arc::clone(&wake));
                    let sender = sender;
                    let _ = sender.send(Stream::open(selected, wake, all_channels));
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
                    let layout_changed = self.sources.iter().enumerate().any(|(lane, settings)| {
                        settings
                            .as_ref()
                            .is_some_and(|settings| settings.source.same_source(&source))
                            && self.formats[lane].as_ref().is_some_and(|old| {
                                let next = stream.format();
                                old.shape() != next.shape()
                                    || (matches!(source, CaptureSource::Sender { .. })
                                        && old.sender_labels() != next.sender_labels())
                            })
                    });
                    if layout_changed
                        && (is_sender(&source) || requests_all_channels(&self.sources, &source))
                    {
                        // A native rate change starts a new history even at the same output rate.
                        // A Sender label change also starts a new history so old audio is never
                        // rendered with the new speaker positions.
                        self.reset_capture();
                    }
                    for (lane, settings) in self.sources.iter().enumerate() {
                        if let Some(settings) = settings
                            && settings.source.same_source(&source)
                        {
                            self.formats[lane] = Some(Arc::clone(stream.format()));
                            self.statuses[lane] = missing_capture_channel(
                                &settings.source,
                                stream.format(),
                                settings.route,
                            );
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
                matches!(
                    self.statuses[lane].as_deref(),
                    None | Some(FALLBACK_TO_MERGED)
                ) && self
                    .inputs
                    .iter()
                    .any(|input| input.source.same_source(&settings.source))
            })
        });
        self.error = if active || !self.pending.is_empty() {
            None
        } else {
            self.statuses
                .iter()
                .zip(&self.sources)
                .filter(|(_, source)| {
                    !source.as_ref().is_some_and(|settings| {
                        matches!(settings.source, CaptureSource::Addon { .. })
                    })
                })
                .find_map(|(status, _)| status.clone())
        };
    }

    fn positioned_lanes(&self) -> [Option<usize>; 2] {
        std::array::from_fn(|lane| {
            let settings = self.sources[lane].as_ref()?;
            let source = self.addon_source(&settings.source)?;
            source
                .groups
                .as_ref()?
                .iter()
                .any(|group| group.positioned)
                .then_some(self.capture_lanes[lane])
                .flatten()
        })
    }

    fn configure_capture(&mut self) {
        let channels = |lane: usize| {
            let source = self.sources[lane].as_ref()?;
            capture_channels(&source.source, self.formats[lane].as_ref()?, source.route)
        };
        let mut widths = [channels(0), channels(1)];
        for (lane, source) in self.sources.iter().enumerate() {
            if let Some(source) = source
                && let Some(format) = &self.formats[lane]
            {
                let missing = missing_capture_channel(&source.source, format, source.route);
                if missing.is_some()
                    || self.layout_blocked[lane]
                    || self
                        .inputs
                        .iter()
                        .any(|input| input.source.same_source(&source.source))
                {
                    self.statuses[lane] = missing;
                }
            }
        }
        self.layout_blocked = [false; 2];
        for lane in 0..2 {
            if widths[lane].is_none() {
                continue;
            }
            match self.groups_for_lane(lane) {
                Ok(groups) => self.groups[lane] = groups,
                Err(reason) => {
                    self.statuses[lane] = Some(reason);
                    self.layout_blocked[lane] = true;
                    widths[lane] = None;
                }
            }
        }
        if let Some(a) = widths[0]
            && let Err(reason) = combined_channels(a, widths[1])
        {
            self.statuses[1] = Some(reason);
            self.layout_blocked[1] = true;
            widths[1] = None;
        }
        let previous_lanes = self.capture_lanes;
        let mut physical = 0;
        self.capture_lanes = widths.map(|width| {
            width.map(|_| {
                let lane = physical;
                physical += 1;
                lane
            })
        });
        let active: Vec<_> = widths
            .iter()
            .enumerate()
            .filter_map(|(lane, width)| width.map(|width| (lane, width)))
            .collect();
        let Some(&(first_lane, first_width)) = active.first() else {
            self.reset_capture();
            return;
        };
        let sample_rate = if self.shared_source_routes().is_some() {
            self.formats[first_lane]
                .as_ref()
                .map_or(COMMON_RATE, |format| format.sample_rate)
        } else {
            COMMON_RATE
        };
        let mut shape = (
            sample_rate,
            first_width,
            active.get(1).map(|&(_, width)| width),
        );
        if self.shape == Some(shape) && previous_lanes == self.capture_lanes {
            return;
        }
        let mut capture = SynchronizedCapture::new_with_budget(
            shape.0,
            shape.1,
            shape.2,
            self.retention,
            self.history_budget_samples,
        );
        if matches!(
            capture,
            Err(CaptureError::BufferTooLarge | CaptureError::AllocationFailed)
        ) && shape.0 != COMMON_RATE
        {
            // Very high native rates can exceed the maximum history allocation.
            shape.0 = COMMON_RATE;
            if self.shape == Some(shape) && previous_lanes == self.capture_lanes {
                return;
            }
            capture = SynchronizedCapture::new_with_budget(
                shape.0,
                shape.1,
                shape.2,
                self.retention,
                self.history_budget_samples,
            );
        }
        match capture {
            Ok(mut capture) => {
                for physical in self.positioned_lanes().into_iter().flatten() {
                    if let Err(error) = capture.enable_frame_clock(physical) {
                        self.error = Some(error.to_string());
                        self.reset_capture();
                        return;
                    }
                }
                capture.set_retain_silence(self.retain_silence);
                if self.paused {
                    capture.capture_mut().pause();
                }
                self.capture = Some(capture);
                self.clear_positions();
                self.shape = Some(shape);
                self.bump_generation();
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    fn reset_capture(&mut self) {
        self.capture = None;
        self.clear_positions();
        self.shape = None;
        self.bump_generation();
    }

    fn bump_generation(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.revision = self.revision.wrapping_add(1);
    }

    fn clear_positions(&mut self) {
        for positions in &mut self.positions {
            positions.clear();
        }
        self.position_overflowed = [false; 2];
    }

    fn shared_source_routes(&self) -> Option<([ChannelRoute; 2], usize)> {
        let mut active = self
            .sources
            .iter()
            .enumerate()
            .filter_map(|(lane, source)| self.capture_lanes[lane].and(source.as_ref()));
        let a = active.next()?;
        match active.next() {
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
                if matches!(input.stream, Stream::Sender { .. }) {
                    self.sender_packet_at = Some(Instant::now());
                }
                let format = input.stream.format();
                if !self.sources.iter().enumerate().any(|(lane, source)| {
                    self.capture_lanes[lane].is_some()
                        && source
                            .as_ref()
                            .is_some_and(|source| source.source.same_source(&input.source))
                }) {
                    continue;
                }
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
                    if is_sender(&input.source) && format.channels > 2 {
                        self.scratch.clear();
                        for frame in samples.chunks_exact(format.channels) {
                            for _ in 0..count {
                                self.scratch.extend_from_slice(frame);
                            }
                        }
                    } else {
                        let routes = routes.map(|route| effective_route(format, route));
                        format.route_tracks(&routes[..count], samples, &mut self.scratch);
                    }
                    if let Err(error) = capture.push_native_interleaved_with_markers(
                        timestamp_ns,
                        &self.scratch,
                        packet.discontinuity,
                        &self.bars,
                    ) {
                        for (lane, status) in self.statuses.iter_mut().enumerate() {
                            if self.capture_lanes[lane].is_some() {
                                *status = Some(error.to_string());
                            }
                        }
                    }
                    continue;
                }
                for (lane, source) in self.sources.iter().enumerate() {
                    let Some(physical_lane) = self.capture_lanes[lane] else {
                        continue;
                    };
                    let Some(source) = source
                        .as_ref()
                        .filter(|source| source.source.same_source(&input.source))
                    else {
                        continue;
                    };
                    route_samples(
                        &source.source,
                        format,
                        source.route,
                        samples,
                        &mut self.scratch,
                    );
                    if !self.scratch.is_empty()
                        && let Err(error) = capture.push_with_markers(
                            physical_lane,
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
        let frames = capture.capture().frame_range();
        for (lane, positions) in self.positions.iter_mut().enumerate() {
            if let Some(physical) = self.capture_lanes[lane] {
                positions.resolve_capture(capture, physical);
                if let Some(first) = capture.take_frame_clock_eviction(physical) {
                    positions.clock_evicted_before(first);
                }
            }
            positions.retain_frames(frames.clone());
            self.position_overflowed[lane] |= positions.take_overflowed();
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
        for positions in &mut self.positions { positions.stop_pending(); }
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

    /// Pins the requested frames of every active lane. The lanes share the capture's
    /// one-second pages, so while the capture lock is held this clones one pointer per
    /// retained second and copies only the page still being written.
    fn snapshots(&self, selection: Option<Range<u64>>) -> Option<Arc<[FrozenTrack]>> {
        let capture = self.capture.as_ref()?.capture();
        let frames = selection.unwrap_or_else(|| capture.frame_range());
        if frames.is_empty() {
            return None;
        }
        let count = if capture.track_channels(1).is_some() {
            2
        } else {
            1
        };
        let frozen = capture.freeze_range(frames)?;
        let tracks = (0..count)
            .map(|lane| frozen.track(lane))
            .collect::<Option<Vec<_>>>()?;
        Some(tracks.into())
    }

    fn edit_snapshot(&self) -> Option<EditSnapshot> {
        let capture = self.capture.as_ref()?.capture();
        let frames = capture.frame_range();
        Some(EditSnapshot {
            snapshots: self.snapshots(Some(frames.clone()))?,
            positions: self.positions.each_ref().map(PositionTimeline::freeze),
            bar_markers: capture.bar_markers(frames),
            generation: self.generation,
            groups: self.groups.clone(),
            shown_groups: self.current_shown_groups(),
            lanes: self.active_lanes(),
        })
    }

    fn clear(&mut self) {
        self.discard_before_ns = now_ns();
        for input in &mut self.inputs {
            input
                .stream
                .discard_pending(&mut self.native, &mut self.bars);
        }
        if let Some(capture) = &mut self.capture {
            capture.reset_input();
            capture.capture_mut().clear();
        }
        self.clear_positions();
        self.bump_generation();
    }

    fn set_retention(&mut self, retention: DurationChoice) {
        self.retention = retention;
        let lanes = self.positioned_lanes();
        if let Some(capture) = &mut self.capture {
            let before = capture.capture().revision();
            capture.capture_mut().set_retention(retention);
            for physical in lanes.into_iter().flatten() {
                if let Err(error) = capture.enable_frame_clock(physical) {
                    self.error = Some(error.to_string());
                }
            }
            if capture.capture().revision() != before {
                self.revision = self.revision.wrapping_add(1);
            }
        }
        if let Some(capture) = &self.capture {
            let frames = capture.capture().frame_range();
            for positions in &mut self.positions {
                positions.retain_frames(frames.clone());
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
            self.mode_error = failures
                .into_iter()
                .next()
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

/// Positions use the captured lane's channel indices, after narrow source routing.
fn positioned_lane_channel(
    source: &backtracking_addon::protocol::Source,
    route: ChannelRoute,
    channel: u32,
) -> Option<u32> {
    let mut offset = 0;
    let mut positioned = false;
    for group in source.groups.iter().flatten() {
        let end = offset + group.channels.len();
        if (offset..end).contains(&(channel as usize)) {
            positioned = group.positioned;
        }
        offset = end;
    }
    if !positioned {
        return None;
    }
    if offset > 2 {
        return Some(channel);
    }
    match route {
        ChannelRoute::Merged => Some(channel),
        ChannelRoute::Left if channel == 0 => Some(0),
        ChannelRoute::Right if channel == 1 => Some(0),
        // Mid and Side combine two positions into one sample; neither has a
        // single source position. Center does not exist in a narrow stream.
        _ => None,
    }
}

const FALLBACK_TO_MERGED: &str =
    "All channels is unavailable. Using Merged. / 目前無法擷取所有聲道，已改用合併聲道。";

fn missing_channel(format: &Format, route: ChannelRoute) -> Option<String> {
    if effective_route(format, route) != route {
        return Some(FALLBACK_TO_MERGED.into());
    }
    format
        .route_channels(route)
        .is_none()
        .then(|| "目前的來源沒有提供所選聲道".into())
}

fn missing_capture_channel(
    source: &CaptureSource,
    format: &Format,
    route: ChannelRoute,
) -> Option<String> {
    if is_sender(source) && format.channels > 2 {
        None
    } else {
        missing_channel(format, route)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sender_groups_use_labels_only_on_wide_lanes() {
        let labels = [
            "FL", "FR", "FC", "LFE", "Lb", "Rb", "Ls", "Rs", "TFL", "TFR", "TBL", "TBR",
        ];
        let mut sender = backtracking_core::SenderEndpoint::new().unwrap();
        let source = CaptureSource::Sender {
            id: sender.id().into(),
            name: "Sender groups".into(),
        };
        let mut session = CaptureSession::new();
        session.sources[1] = Some(SourceSettings {
            source: source.clone(),
            route: ChannelRoute::Merged,
        });
        for (channels, names, expected) in [
            (12, labels.as_slice(), labels.map(str::to_owned).to_vec()),
            (12, &[], (1..=12).map(|i| format!("Channel {i}")).collect()),
            (
                2,
                &labels[..2],
                vec!["Channel 1".into(), "Channel 2".into()],
            ),
        ] {
            sender
                .update_with_labels("Sender groups", true, 48_000, channels, names)
                .unwrap();
            let stream = Stream::open(source.clone(), Arc::new(|| {}), false).unwrap();
            session.formats[1] = Some(Arc::clone(stream.format()));
            let groups = session.groups_for_lane(1).unwrap();
            assert_eq!(groups.len(), 1);
            assert_eq!(groups[0].channels, 0..channels);
            assert_eq!(groups[0].labels, expected);
            assert!(!groups[0].positioned);
        }
    }

    #[test]
    fn connected_sender_reopens_for_new_labels_and_frozen_audio_keeps_its_layout() {
        let old = [
            "FL", "FR", "FC", "LFE", "Ls", "Rs", "TFL", "TFR", "TBL", "TBR",
        ];
        let new = [
            "FL", "FR", "FC", "LFE", "Lb", "Rb", "Ls", "Rs", "TFL", "TFR",
        ];
        let mut sender = backtracking_core::SenderEndpoint::new().unwrap();
        sender
            .update_with_labels("Ten channels", true, 48_000, 10, &old)
            .unwrap();
        let mut session = CaptureSession::new();
        session.history_budget_samples = 48_000;
        session.paused = false;
        session.set_sources([
            Some(SourceSettings {
                source: CaptureSource::Sender {
                    id: sender.id().into(),
                    name: "Ten channels".into(),
                },
                route: ChannelRoute::Merged,
            }),
            None,
        ]);
        let pump_until_recording =
            |sender: &mut backtracking_core::SenderEndpoint, session: &mut CaptureSession| {
                let deadline = Instant::now() + Duration::from_secs(10);
                loop {
                    sender
                        .publish("Ten channels", 48_000, 10, now_ns(), &[0.25; 4800], &[])
                        .unwrap();
                    session.poll();
                    session.drain(true);
                    if !session.inputs.is_empty()
                        && let Some(snapshot) = session.edit_snapshot()
                    {
                        return snapshot;
                    }
                    assert!(
                        Instant::now() < deadline,
                        "Sender failed to record: {:?}",
                        session.error
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
            };
        let frozen = pump_until_recording(&mut sender, &mut session);
        assert_eq!(frozen.groups[0][0].labels, old);
        sender
            .update_with_labels("Ten channels", true, 48_000, 10, &new)
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while session.groups[0][0].labels != new {
            session.poll();
            assert!(
                Instant::now() < deadline,
                "Sender did not reconnect: {:?}",
                session.error
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(session.generation > frozen.generation);
        assert!(session.edit_snapshot().is_none());
        let resumed = pump_until_recording(&mut sender, &mut session);
        assert_eq!(resumed.groups[0][0].labels, new);
        assert_eq!(frozen.groups[0][0].labels, old);
        assert!(!frozen.snapshots[0].frame_range().is_empty());
    }

    fn addon_fixture(state: AddonState) -> AddonSnapshot {
        use backtracking_addon::protocol::{ChannelGroup, Format as AddonFormat, Source};
        AddonSnapshot {
            revision: 1,
            error: None,
            addons: vec![crate::addons::AddonInfo {
                id: "test.addon".into(),
                manifest: None,
                enabled: true,
                state,
                pid: None,
                message: None,
                settings: Vec::new(),
                actions: Vec::new(),
                sources: vec![Source {
                    id: "main".into(),
                    name: "Six channels".into(),
                    sender: "abc-123".into(),
                    formats: vec![AddonFormat {
                        id: "5.1".into(),
                        label: "5.1".into(),
                    }],
                    format: "5.1".into(),
                    transport: None,
                    groups: Some(vec![ChannelGroup {
                        id: "bed".into(),
                        label: "Bed".into(),
                        channels: ["L", "R", "C", "LFE", "Ls", "Rs"]
                            .map(str::to_string)
                            .into(),
                        positioned: false,
                    }]),
                }],
            }],
        }
    }

    fn addon_selection() -> CaptureSource {
        CaptureSource::Addon {
            addon_id: "test.addon".into(),
            source_id: "main".into(),
            name: "Six channels".into(),
        }
    }

    fn activity_fixture(width: usize, rate: u32, seconds: usize, warm_peaks: bool) -> Controller {
        let mut controller = Controller::new().unwrap();
        let mut capture = SynchronizedCapture::new_with_budget(
            rate,
            width,
            None,
            DurationChoice::Minutes10,
            rate as usize * width * seconds,
        )
        .unwrap();
        capture.set_retain_silence(true);
        let frames = rate as usize / 10;
        let mut packet = vec![0.; frames * width];
        packet[0] = 0.25;
        for i in 0..seconds * 10 {
            capture
                .push_native_interleaved(i as u64 * 100_000_000, &packet, false)
                .unwrap();
        }
        if warm_peaks {
            let track = capture.capture().freeze().track(0).unwrap();
            let start = Instant::now();
            for i in 0..width {
                std::hint::black_box(track.channel_extrema(i, track.frame_range()));
            }
            eprintln!("built all native peaks in {:?}", start.elapsed());
        }
        let mut state = lock(&controller.state);
        state.capture = Some(capture);
        state.shape = Some((rate, width, None));
        state.capture_lanes = [Some(0), None];
        state.revision = 1;
        controller.source_status = SourceStatus::from_session(&state);
        drop(state);
        controller
    }

    #[test]
    fn activity_keeps_last_mask_while_waveform_worker_refreshes() {
        let mut controller = activity_fixture(8, 8000, 1, false);
        let (wake, ready) = mpsc::channel();
        controller.set_repaint(Arc::new(move || {
            let _ = wake.send(());
        }));
        let state = lock(&controller.state);
        assert_eq!(controller.try_active_channels(0), None);
        drop(state);
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(controller.try_active_channels(0), Some(1));
        let mut state = lock(&controller.state);
        let capture = state.capture.as_mut().unwrap();
        let mut frame = [0.; 8];
        frame[4] = 0.1;
        capture
            .push_native_interleaved(1_000_000_000, &frame, false)
            .unwrap();
        state.revision += 1;
        controller.source_status = SourceStatus::from_session(&state);
        assert_eq!(controller.try_active_channels(0), Some(1));
        drop(state);
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(controller.try_active_channels(0), Some(17));
        let mut state = lock(&controller.state);
        state.generation += 1;
        controller.source_status = SourceStatus::from_session(&state);
        assert_eq!(controller.try_active_channels(0), None);
    }

    #[test]
    #[ignore = "full-history waveform worker activity and window latency"]
    fn activity_worker_cost() {
        for (width, warm) in [(8, false), (12, false), (32, false), (12, true)] {
            let controller = activity_fixture(width, 48000, 600, warm);
            let (wake, ready) = mpsc::channel();
            controller.set_repaint(Arc::new(move || {
                let _ = wake.send(());
            }));
            let start = Instant::now();
            assert_eq!(controller.try_active_channels(0), None);
            let enqueue = start.elapsed();
            ready.recv_timeout(Duration::from_secs(120)).unwrap();
            assert_eq!(controller.try_active_channels(0), Some(1));
            let start = Instant::now();
            for _ in 0..10000 {
                assert_eq!(controller.try_active_channels(0), Some(1));
            }
            let cached = start.elapsed() / 10000;
            let reads = controller.activity.lock().unwrap();
            eprintln!(
                "width={width} existing_peaks={warm} window_enqueue={enqueue:?} worker={:?} window_cached={cached:?}",
                reads[0].worker_time
            );
        }
    }

    #[test]
    fn saved_native_route_follows_reopened_stereo_format() {
        let source = CaptureSource::System;
        let route = ChannelRoute::AllChannels;
        let wide = Format::sender(48000, 8);
        let stereo = Format::sender(48000, 2);
        assert_eq!(capture_channels(&source, &wide, route), Some(8));
        assert_eq!(capture_channels(&source, &stereo, route), Some(2));
        assert!(missing_capture_channel(&source, &wide, route).is_none());
        assert!(
            missing_capture_channel(&source, &stereo, route)
                .unwrap()
                .contains("Merged")
        );
        let mut result = Vec::new();
        route_samples(&source, &stereo, route, &[0.25, -0.5], &mut result);
        assert_eq!(result, [0.25, -0.5]);
    }

    #[test]
    fn reopening_the_same_system_source_reconfigures_the_capture() {
        let mut session = CaptureSession::new();
        session.paused = false;
        session.history_budget_samples = 48000;
        session.sources = [
            Some(SourceSettings {
                source: CaptureSource::System,
                route: ChannelRoute::AllChannels,
            }),
            None,
        ];
        for width in [8, 2, 8] {
            session.inputs.clear();
            let (tx, result) = mpsc::sync_channel(1);
            tx.send(Ok(Stream::Sender {
                receiver: None,
                format: Arc::new(Format::sender(48000, width)),
                id: String::new(),
                error: None,
            }))
            .unwrap();
            session.pending.push(PendingInput {
                source: CaptureSource::System,
                result,
            });
            let before = session.generation;
            session.poll_pending();
            assert!(session.generation > before);
            session.update_error();
            assert!(session.error.is_none());
            let capture = session.capture.as_mut().unwrap();
            assert_eq!(capture.capture().track_channels(0), Some(width));
            capture
                .push_native_interleaved(0, &vec![0.25; width * 10], false)
                .unwrap();
            assert_eq!(capture.capture().frame_range(), 0..10);
            assert_eq!(
                session.statuses[0]
                    .as_deref()
                    .is_some_and(|s| s.contains("Merged")),
                width == 2
            );
        }
    }

    #[test]
    fn positioned_channels_follow_narrow_routes_and_native_wide_lanes() {
        let mut snapshot = addon_fixture(AddonState::Running);
        let source = &mut snapshot.addons[0].sources[0];
        source.groups.as_mut().unwrap()[0].positioned = true;
        assert_eq!(
            positioned_lane_channel(source, ChannelRoute::Right, 5),
            Some(5)
        );
        source.groups.as_mut().unwrap()[0].channels.truncate(2);
        assert_eq!(
            positioned_lane_channel(source, ChannelRoute::Right, 1),
            Some(0)
        );
        assert_eq!(
            positioned_lane_channel(source, ChannelRoute::Right, 0),
            None
        );
        assert_eq!(
            positioned_lane_channel(source, ChannelRoute::Left, 0),
            Some(0)
        );
        assert_eq!(
            positioned_lane_channel(source, ChannelRoute::Merged, 1),
            Some(1)
        );
        assert_eq!(positioned_lane_channel(source, ChannelRoute::Mid, 0), None);
        assert_eq!(positioned_lane_channel(source, ChannelRoute::Side, 1), None);
    }

    #[test]
    fn removing_positioned_stops_new_states_but_keeps_frozen_history() {
        use backtracking_addon::protocol::ChannelPosition;
        let mut session = retained_addon_session();
        let mut description = addon_fixture(AddonState::Running);
        description.revision = 2;
        description.addons[0].sources[0].groups.as_mut().unwrap()[0].positioned = true;
        session.apply_addon_snapshot(Arc::new(description.clone()));
        session.positions[0].audio_packet(1_000_000_000, 100, 1000, 1000, 0..100);
        let mut message = AddonPositions {
            sequence: 1,
            addon_id: "test.addon".into(),
            source_id: "main".into(),
            time: 1_010_000_000,
            channels: vec![ChannelPosition::new(2, -0.5, 0.5, 0.0)],
        };
        session.receive_positions(&message);
        message.time = 2_010_000_000;
        session.receive_positions(&message);
        description.revision = 3;
        description.addons[0].sources[0].groups.as_mut().unwrap()[0].channels[2] = "Renamed".into();
        session.apply_addon_snapshot(Arc::new(description.clone()));
        session.positions[0].audio_packet(2_000_000_000, 100, 1000, 1000, 0..200);
        assert_eq!(
            session.positions[0].freeze().len(),
            2,
            "label changes must preserve future states"
        );
        let frozen = session.positions[0].freeze();
        description.revision = 4;
        description.addons[0].sources[0].groups.as_mut().unwrap()[0].positioned = false;
        session.apply_addon_snapshot(Arc::new(description));
        message.time = 2_020_000_000;
        session.receive_positions(&message);
        assert_eq!(session.positions[0].freeze().len(), 2);
        assert_eq!(frozen.len(), 2);
        session.clear();
        assert!(session.positions[0].freeze().is_empty());
        assert_eq!(frozen.len(), 2);
    }

    #[test]
    fn addon_identity_and_source_list_do_not_duplicate_its_sender() {
        let snapshot = addon_fixture(AddonState::Running);
        let source = addon_selection();
        assert!(source.same_source(&CaptureSource::Addon {
            addon_id: "test.addon".into(),
            source_id: "main".into(),
            name: "New name".into(),
        }));
        let listed = sources_with_addons(
            vec![CaptureSource::System, CaptureSource::DefaultMicrophone],
            vec![
                SenderInfo {
                    id: "abc-123".into(),
                    name: "Unlabelled".into(),
                    sample_rate: 48000,
                    channels: 6,
                },
                SenderInfo {
                    id: "def-456".into(),
                    name: "Other".into(),
                    sample_rate: 48000,
                    channels: 2,
                },
            ],
            &snapshot,
        );
        assert_eq!(listed.len(), 4);
        assert_eq!(listed[2], source);
        assert!(matches!(&listed[3], CaptureSource::Sender { id, .. } if id == "def-456"));
    }

    fn retained_addon_session() -> CaptureSession {
        let mut session = CaptureSession::new();
        session.sources = [
            Some(SourceSettings {
                source: addon_selection(),
                route: ChannelRoute::Merged,
            }),
            Some(SourceSettings {
                source: CaptureSource::System,
                route: ChannelRoute::Merged,
            }),
        ];
        session.addon_snapshot = Some(Arc::new(addon_fixture(AddonState::Running)));
        session.formats = [
            Some(Arc::new(Format::sender(48_000, 6))),
            Some(Arc::new(Format::sender(48_000, 2))),
        ];
        session.history_budget_samples = 1024;
        session.paused = false;
        session.configure_capture();
        session.inputs.push(Input {
            source: addon_selection(),
            stream: Stream::Sender {
                receiver: None,
                format: Arc::clone(session.formats[0].as_ref().unwrap()),
                id: "abc-123".into(),
                error: None,
            },
        });
        session
            .capture
            .as_mut()
            .unwrap()
            .push_native_interleaved_with_markers(0, &[0.5; 80], false, &[])
            .unwrap();
        session
    }

    fn retained_samples(session: &CaptureSession) -> [Vec<f32>; 2] {
        std::array::from_fn(|lane| {
            session
                .capture
                .as_ref()
                .unwrap()
                .capture()
                .track_snapshot(lane)
                .unwrap()
                .samples
        })
    }

    fn reopen_retained_sender(session: &mut CaptureSession, rate: u32, channels: usize) {
        let (reply, result) = mpsc::sync_channel(1);
        reply
            .send(Ok(Stream::Sender {
                receiver: None,
                format: Arc::new(Format::sender(rate, channels)),
                id: "abc-123".into(),
                error: None,
            }))
            .unwrap();
        session.pending.push(PendingInput {
            source: session.sources[0].as_ref().unwrap().source.clone(),
            result,
        });
        session.poll_pending();
    }

    #[test]
    fn addon_disappearance_and_return_preserve_both_lanes() {
        for state in [
            AddonState::Stopped,
            AddonState::Starting,
            AddonState::Failed("crashed".into()),
        ] {
            let mut session = retained_addon_session();
            let samples = retained_samples(&session);
            let generation = session.generation;
            let groups = session.groups.clone();
            let mut snapshot = addon_fixture(state);
            snapshot.revision = 2;
            snapshot.addons[0].sources.clear();
            session.apply_addon_snapshot(Arc::new(snapshot));
            assert_eq!(retained_samples(&session), samples);
            assert_eq!(session.generation, generation);
            assert_eq!(session.groups, groups);
            assert!(session.inputs.is_empty());
            assert!(session.statuses[0].is_some());
            // Reconfiguring another input must not erase the missing source's status.
            session.configure_capture();
            assert!(session.statuses[0].is_some());
            let mut snapshot = addon_fixture(AddonState::Running);
            snapshot.revision = 3;
            snapshot.addons[0].sources[0].sender = "abc-124".into();
            session.apply_addon_snapshot(Arc::new(snapshot));
            reopen_retained_sender(&mut session, 48_000, 6);
            assert_eq!(retained_samples(&session), samples);
            assert_eq!(session.generation, generation);
            assert!(session.statuses[0].is_none());
            session
                .capture
                .as_mut()
                .unwrap()
                .push_native_interleaved_with_markers(208_334, &[0.25; 80], false, &[])
                .unwrap();
            let resumed = retained_samples(&session);
            for lane in 0..2 {
                assert!(resumed[lane].len() > samples[lane].len());
                assert_eq!(&resumed[lane][..samples[lane].len()], &samples[lane]);
            }
        }
    }

    #[test]
    fn addon_group_and_format_labels_preserve_both_lanes() {
        let mut session = retained_addon_session();
        let samples = retained_samples(&session);
        let generation = session.generation;
        let mut snapshot = addon_fixture(AddonState::Running);
        snapshot.revision = 2;
        snapshot.addons[0].sources[0].groups.as_mut().unwrap()[0].channels[0] = "Left".into();
        session.apply_addon_snapshot(Arc::new(snapshot.clone()));
        assert_eq!(retained_samples(&session), samples);
        assert_eq!(session.generation, generation);
        assert_eq!(session.groups[0][0].labels[0], "Left");
        assert_eq!(session.inputs.len(), 1);
        snapshot.revision = 3;
        snapshot.addons[0].sources[0].format = "renamed".into();
        snapshot.addons[0].sources[0].formats[0].id = "renamed".into();
        session.apply_addon_snapshot(Arc::new(snapshot));
        reopen_retained_sender(&mut session, 48_000, 6);
        assert_eq!(retained_samples(&session), samples);
        assert_eq!(session.generation, generation);
    }

    #[test]
    fn missing_sender_registration_preserves_both_lanes_until_real_format_change() {
        for (rate, channels) in [(96_000, 6), (48_000, 8)] {
            let mut session = retained_addon_session();
            session.sources[0].as_mut().unwrap().source = CaptureSource::Sender {
                id: "abc-123".into(),
                name: "Sender".into(),
            };
            session.inputs[0].source = session.sources[0].as_ref().unwrap().source.clone();
            let samples = retained_samples(&session);
            let generation = session.generation;
            session.check_sender_layouts();
            assert_eq!(retained_samples(&session), samples);
            assert_eq!(session.generation, generation);
            assert!(session.inputs.is_empty());
            assert!(
                session.statuses[0]
                    .as_ref()
                    .unwrap()
                    .contains("unavailable")
            );
            reopen_retained_sender(&mut session, 48_000, 6);
            assert_eq!(retained_samples(&session), samples);
            assert_eq!(session.generation, generation);
            assert!(session.statuses[0].is_none());
            session.inputs.clear();
            reopen_retained_sender(&mut session, rate, channels);
            assert!(session.generation > generation);
            assert!(retained_samples(&session).iter().all(Vec::is_empty));
        }
    }

    #[test]
    fn two_sender_connections_keep_counted_packets_and_fade_jittered_joins() {
        for jitter in [false, true] {
            let mut session = CaptureSession::new();
            session.paused = false;
            session.history_budget_samples = 48000;
            session.discard_before_ns = 0;
            let mut endpoints: [backtracking_core::SenderEndpoint; 2] =
                std::array::from_fn(|lane| {
                    let mut endpoint = backtracking_core::SenderEndpoint::new().unwrap();
                    endpoint.update("Packet clock test", true, 48000, 1);
                    let source = CaptureSource::Sender {
                        id: endpoint.id().into(),
                        name: "Packet clock test".into(),
                    };
                    let stream = Stream::open(source.clone(), Arc::new(|| {}), false).unwrap();
                    session.formats[lane] = Some(Arc::clone(stream.format()));
                    session.sources[lane] = Some(SourceSettings {
                        source: source.clone(),
                        route: ChannelRoute::Merged,
                    });
                    session.inputs.push(Input { source, stream });
                    endpoint
                });
            session.configure_capture();
            let samples: Vec<_> = (0..480).map(|i| 0.2 + i as f32 / 1000.).collect();
            for i in 0..20 {
                for (lane, endpoint) in endpoints.iter_mut().enumerate() {
                    let time = 1_000_000_000
                        + i * 10_000_000
                        + if jitter && lane == 0 {
                            i % 2 * 1_000_000
                        } else {
                            0
                        };
                    endpoint
                        .publish("Packet clock test", 48000, 1, time, &samples, &[])
                        .unwrap();
                }
                session.drain(false);
            }
            let deadline = Instant::now() + Duration::from_secs(2);
            while session
                .capture
                .as_ref()
                .unwrap()
                .capture()
                .frame_range()
                .end
                < 19 * 480
            {
                assert!(
                    Instant::now() < deadline,
                    "last Sender packet was not received"
                );
                std::thread::yield_now();
                session.drain(false);
            }
            session.drain(true);
            let audio = session
                .capture
                .as_ref()
                .unwrap()
                .capture()
                .track_snapshot(0)
                .unwrap()
                .samples;
            if jitter {
                for i in 1..19 {
                    let start = i * 480 + 48;
                    assert_eq!(audio[start], 0.);
                    assert_eq!(audio[start + 100], samples[100]);
                    assert_eq!(audio[start + 479], 0.);
                }
            } else {
                assert_eq!(audio, samples.repeat(20));
            }
        }
    }

    #[test]
    fn sender_audio_switched_off_preserves_both_lanes() {
        let mut endpoint = backtracking_core::SenderEndpoint::new().unwrap();
        endpoint.update("Retention test", true, 48_000, 6);
        let mut session = retained_addon_session();
        session.sources[0].as_mut().unwrap().source = CaptureSource::Sender {
            id: endpoint.id().into(),
            name: "Sender".into(),
        };
        session.inputs[0].source = session.sources[0].as_ref().unwrap().source.clone();
        if let Stream::Sender { id, .. } = &mut session.inputs[0].stream {
            *id = endpoint.id().into();
        }
        let samples = retained_samples(&session);
        let generation = session.generation;
        session.check_sender_layouts();
        assert_eq!(session.inputs.len(), 1);
        endpoint.update("Retention test", false, 48_000, 6);
        session.next_layout_check = Instant::now();
        session.check_sender_layouts();
        assert!(session.inputs.is_empty());
        assert_eq!(retained_samples(&session), samples);
        assert_eq!(session.generation, generation);
        assert!(session.statuses[0].is_some());
    }

    #[test]
    fn stopped_addon_has_status_without_repeated_connection_attempts() {
        let mut session = CaptureSession::new();
        session.sources = [
            Some(SourceSettings {
                source: addon_selection(),
                route: ChannelRoute::Merged,
            }),
            None,
        ];
        session.addon_snapshot = Some(Arc::new(addon_fixture(AddonState::Stopped)));
        session.paused = false;
        assert!(!session.source_missing());
        session.request_missing();
        assert!(session.pending.is_empty());
        assert_eq!(session.statuses[0].as_deref(), Some("Add-on is stopped"));
        session.update_error();
        assert!(session.error.is_none());
    }

    #[test]
    fn wide_sender_ignores_saved_stereo_route_and_preserves_native_channels() {
        let mut session = CaptureSession::new();
        let source = addon_selection();
        session.sources = [
            Some(SourceSettings {
                source: source.clone(),
                route: ChannelRoute::Left,
            }),
            None,
        ];
        session.addon_snapshot = Some(Arc::new(addon_fixture(AddonState::Running)));
        session.formats[0] = Some(Arc::new(Format::sender(10, 6)));
        session.history_budget_samples = 120;
        session.configure_capture();
        assert_eq!(session.shape, Some((10, 6, None)));
        assert_eq!(session.groups[0][0].channels, 0..6);
        assert_eq!(
            session.groups[0][0].labels,
            ["L", "R", "C", "LFE", "Ls", "Rs"]
        );
        let input = [0.1, 0.2, 0.3, 0.4, 0.5, 0.6];
        let mut routed = Vec::new();
        route_samples(
            &source,
            session.formats[0].as_ref().unwrap(),
            ChannelRoute::Left,
            &input,
            &mut routed,
        );
        assert_eq!(routed, input);
        assert_eq!(
            session
                .capture
                .as_ref()
                .unwrap()
                .capture()
                .effective_retention_seconds(),
            2.0
        );
    }

    #[test]
    fn second_lane_over_64_is_refused_without_allocating_its_channels() {
        let mut session = CaptureSession::new();
        session.sources = ["abc-1", "abc-2"].map(|id| {
            Some(SourceSettings {
                source: CaptureSource::Sender {
                    id: id.into(),
                    name: id.into(),
                },
                route: ChannelRoute::Merged,
            })
        });
        session.formats = [
            Some(Arc::new(Format::sender(48_000, 40))),
            Some(Arc::new(Format::sender(48_000, 25))),
        ];
        session.history_budget_samples = 640;
        session.configure_capture();
        assert_eq!(session.shape, Some((48_000, 40, None)));
        assert!(
            session.statuses[1]
                .as_ref()
                .unwrap()
                .contains("limit is 64")
        );
        assert!(
            session
                .capture
                .as_ref()
                .unwrap()
                .capture()
                .track_channels(1)
                .is_none()
        );
        session.formats[1] = Some(Arc::new(Format::sender(48_000, 24)));
        session.configure_capture();
        assert_eq!(session.shape, Some((48_000, 40, Some(24))));
        assert!(session.statuses[1].is_none());
    }

    #[test]
    fn mismatched_addon_groups_do_not_start_capture() {
        let mut session = CaptureSession::new();
        session.sources = [
            Some(SourceSettings {
                source: addon_selection(),
                route: ChannelRoute::Merged,
            }),
            None,
        ];
        session.addon_snapshot = Some(Arc::new(addon_fixture(AddonState::Running)));
        session.formats[0] = Some(Arc::new(Format::sender(10, 12)));
        session.configure_capture();
        assert!(session.capture.is_none());
        assert!(
            session.statuses[0]
                .as_ref()
                .unwrap()
                .contains("Sender has 12")
        );
    }

    #[test]
    fn lane_b_records_while_lane_a_is_unavailable_without_phantom_channels() {
        let mut session = CaptureSession::new();
        session.sources = [
            Some(SourceSettings {
                source: addon_selection(),
                route: ChannelRoute::Merged,
            }),
            Some(SourceSettings {
                source: CaptureSource::Sender {
                    id: "def-1".into(),
                    name: "B".into(),
                },
                route: ChannelRoute::Merged,
            }),
        ];
        session.addon_snapshot = Some(Arc::new(addon_fixture(AddonState::Stopped)));
        session.formats[1] = Some(Arc::new(Format::sender(10, 6)));
        session.history_budget_samples = 120;
        session.paused = false;
        session.configure_capture();
        assert_eq!(session.shape, Some((10, 6, None)));
        assert_eq!(session.capture_lanes, [None, Some(0)]);
        assert_eq!(
            SourceStatus::from_session(&session).channels,
            [None, Some(6)]
        );
        let capture = session.capture.as_mut().unwrap();
        assert_eq!(capture.capture().effective_retention_seconds(), 2.0);
        capture
            .push_native_interleaved_with_markers(0, &[0.5; 60], false, &[])
            .unwrap();
        let frozen = session.edit_snapshot().unwrap();
        assert_eq!(frozen.lanes, vec![1]);
        assert_eq!(frozen.snapshots.len(), 1);
        assert_eq!(frozen.snapshots[0].channels(), 6);
        let mut low = [0.0; 2];
        let mut high = [0.0; 2];
        Controller::render_waveforms_from(
            &session,
            &[(1, WaveformView::Merged)],
            None,
            &mut [(&mut low, &mut high)],
        );
        assert!(high.iter().any(|&peak| peak > 0.0));

        // Reusing the same physical shape for another logical lane must clear old audio.
        session.sources[0] = session.sources[1].clone();
        session.formats[0] = session.formats[1].take();
        session.configure_capture();
        assert_eq!(session.capture_lanes, [Some(0), None]);
        assert!(
            session
                .capture
                .as_ref()
                .unwrap()
                .capture()
                .frame_range()
                .is_empty()
        );
    }

    #[test]
    fn sender_rate_change_resets_independent_lanes_even_with_same_output_rate() {
        let mut session = CaptureSession::new();
        session.sources = ["abc-1", "abc-2"].map(|id| {
            Some(SourceSettings {
                source: CaptureSource::Sender {
                    id: id.into(),
                    name: id.into(),
                },
                route: ChannelRoute::Merged,
            })
        });
        session.formats = [
            Some(Arc::new(Format::sender(44_100, 6))),
            Some(Arc::new(Format::sender(48_000, 2))),
        ];
        session.history_budget_samples = 1024;
        session.paused = false;
        session.configure_capture();
        assert_eq!(session.shape, Some((48_000, 6, Some(2))));
        session
            .capture
            .as_mut()
            .unwrap()
            .push_native_interleaved_with_markers(0, &[0.5; 80], false, &[])
            .unwrap();
        let before = session.generation;
        let (reply, result) = mpsc::sync_channel(1);
        reply
            .send(Ok(Stream::Sender {
                receiver: None,
                format: Arc::new(Format::sender(96_000, 6)),
                id: "abc-1".into(),
                error: None,
            }))
            .unwrap();
        session.pending.push(PendingInput {
            source: session.sources[0].as_ref().unwrap().source.clone(),
            result,
        });
        session.poll_pending();
        assert_eq!(session.shape, Some((48_000, 6, Some(2))));
        assert!(session.generation > before);
        assert!(
            session
                .capture
                .as_ref()
                .unwrap()
                .capture()
                .frame_range()
                .is_empty()
        );
    }

    #[test]
    fn frozen_snapshot_keeps_channel_groups_after_live_layout_changes() {
        let mut session = CaptureSession::new();
        session.sources = [
            Some(SourceSettings {
                source: addon_selection(),
                route: ChannelRoute::Merged,
            }),
            None,
        ];
        session.addon_snapshot = Some(Arc::new(addon_fixture(AddonState::Running)));
        session.formats[0] = Some(Arc::new(Format::sender(10, 6)));
        session.history_budget_samples = 120;
        session.paused = false;
        session.configure_capture();
        session
            .capture
            .as_mut()
            .unwrap()
            .push_native_interleaved_with_markers(0, &[0.5; 60], false, &[])
            .unwrap();
        let frozen = session.edit_snapshot().unwrap();
        session.groups[0][0].labels[0] = "Changed".into();
        assert_eq!(frozen.groups[0][0].labels[0], "L");
        assert_eq!(frozen.shown_groups[0].as_deref(), Some("bed"));
        assert_eq!(frozen.snapshots[0].channels(), 6);
    }

    #[test]
    fn senders_are_listed_right_after_the_fixed_sources() {
        let sender = |id: &str, name: &str| SenderInfo {
            id: id.into(),
            name: name.into(),
            sample_rate: 48_000,
            channels: 2,
        };
        let microphone = |name: &str| CaptureSource::Microphone {
            id: name.into(),
            name: name.into(),
        };
        let devices = vec![
            CaptureSource::System,
            CaptureSource::DefaultMicrophone,
            microphone("Input 1"),
            microphone("Input 2"),
        ];
        let listed = with_senders(
            devices.clone(),
            vec![sender("a-1", "Drums"), sender("b-2", "Bass")],
        );
        assert_eq!(listed.len(), 6);
        assert_eq!(listed[..2], devices[..2]);
        assert_eq!(
            listed[2],
            CaptureSource::Sender {
                id: "a-1".into(),
                name: "【Sender】Drums".into()
            }
        );
        assert_eq!(
            listed[3],
            CaptureSource::Sender {
                id: "b-2".into(),
                name: "【Sender】Bass".into()
            }
        );
        assert_eq!(listed[4..], devices[2..]);
        // Without senders the list is unchanged, and a short list still takes them.
        assert_eq!(with_senders(devices.clone(), Vec::new()), devices);
        assert_eq!(
            with_senders(Vec::new(), vec![sender("a-1", "Drums")]).len(),
            1
        );
    }

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
        lock(&controller.state)
            .capture
            .as_ref()
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
            fn drop(&mut self) {
                let _ = unsafe { CloseHandle(self.0) };
            }
        }

        let pid: u32 = std::env::var("BACKTRACKING_QUIET_TEST_PROCESS_ID")
            .expect("set BACKTRACKING_QUIET_TEST_PROCESS_ID to the isolated silent fixture")
            .parse()
            .expect("fixture PID must be an integer");
        assert!(pid != 0 && pid != std::process::id());
        assert!(
            CaptureSource::supports_app_capture(),
            "Process Loopback is unsupported"
        );
        let handle = FixtureHandle(
            unsafe {
                OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                    false,
                    pid,
                )
            }
            .expect("cannot query the exact fixture process"),
        );
        assert_eq!(unsafe { WaitForSingleObject(handle.0, 0) }, WAIT_TIMEOUT);
        let mut name = vec![0u16; 32_768];
        let mut length = name.len() as u32;
        unsafe {
            QueryFullProcessImageNameW(
                handle.0,
                PROCESS_NAME_WIN32,
                PWSTR(name.as_mut_ptr()),
                &mut length,
            )
        }
        .expect("cannot resolve the exact fixture executable");
        let path = PathBuf::from(String::from_utf16(&name[..length as usize]).unwrap());
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        assert!(
            path.canonicalize()
                .unwrap()
                .starts_with(workspace.join("target").canonicalize().unwrap()),
            "the native test may only capture a fixture under the workspace target directory"
        );
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .to_ascii_lowercase()
                .starts_with("backtracking-quiet"),
            "unexpected fixture executable"
        );
        let path = path.to_string_lossy().into_owned();
        assert_eq!(
            crate::loopback::resolved_process_id_for_test(&path).unwrap(),
            pid,
            "Application source resolution selected a different process"
        );
        eprintln!("native quiet smoke: fixture identity verified");

        let source = CaptureSource::Application {
            path,
            name: "Quiet native fixture".into(),
        };
        let mut controller = Controller::new().unwrap();
        controller.set_sources([
            Some(SourceSettings {
                source: source.clone(),
                route: ChannelRoute::Merged,
            }),
            None,
        ]);
        controller.resume();
        eprintln!("native quiet smoke: dedicated Process Loopback activation requested");
        assert!(
            wait_native_condition(&mut controller, Duration::from_secs(8), |controller| {
                let state = lock(&controller.state);
                state.capture.is_some()
                    && state.inputs.len() == 1
                    && state.inputs[0].source.same_source(&source)
                    && matches!(state.inputs[0].stream, Stream::Wasapi(_))
            }),
            "could not activate the isolated fixture: {:?}",
            controller.error()
        );
        eprintln!("native quiet smoke: dedicated Process Loopback activated");
        assert!(
            controller.sources()[0]
                .as_ref()
                .unwrap()
                .source
                .same_source(&source)
        );
        let rate = lock(&controller.state).shape.unwrap().0;

        thread::sleep(Duration::from_millis(900));
        assert_eq!(
            native_recorded_end(&controller),
            0,
            "default OFF retained idle audio"
        );
        eprintln!("native quiet smoke: default OFF stable");
        controller.set_retain_silence(true);
        assert!(wait_native_condition(
            &mut controller,
            Duration::from_secs(2),
            |controller| lock(&controller.state).retain_silence
        ));
        let on_start = native_recorded_end(&controller);
        let on_clock = Instant::now();
        assert!(
            wait_native_condition(&mut controller, Duration::from_secs(6), |controller| {
                native_recorded_end(controller) >= on_start + u64::from(rate) * 3 / 5
            }),
            "quiet ON did not advance the connected, silent Process Loopback stream"
        );
        let on_elapsed = on_clock.elapsed().as_secs_f64();
        let on_end = native_recorded_end(&controller);
        eprintln!("native quiet smoke: quiet ON advances");
        assert!(
            (on_end - on_start) as f64 <= (on_elapsed + 0.3) * f64::from(rate),
            "quiet ON retroactively retained the earlier OFF idle interval"
        );
        let pinned = controller.request_edit_snapshot().recv().unwrap().unwrap();
        let pinned_range = pinned.snapshots[0].frame_range();
        assert!(pinned_range.end >= on_end);
        assert!(
            pinned
                .snapshots
                .iter()
                .flat_map(|track| track.snapshot().samples)
                .all(|sample| sample == 0.0),
            "the isolated silent fixture unexpectedly produced nonzero audio"
        );

        controller.set_retain_silence(false);
        assert!(wait_native_condition(
            &mut controller,
            Duration::from_secs(2),
            |controller| !lock(&controller.state).retain_silence
        ));
        let off_end = native_recorded_end(&controller);
        thread::sleep(Duration::from_millis(700));
        assert_eq!(
            native_recorded_end(&controller),
            off_end,
            "quiet OFF kept advancing"
        );
        eprintln!("native quiet smoke: quiet OFF stable");

        controller.pause();
        controller.set_retain_silence(true);
        assert!(wait_native_condition(
            &mut controller,
            Duration::from_secs(2),
            |controller| lock(&controller.state).retain_silence
        ));
        let paused_end = native_recorded_end(&controller);
        thread::sleep(Duration::from_millis(650));
        assert_eq!(
            native_recorded_end(&controller),
            paused_end,
            "manual Pause recorded quiet audio"
        );
        assert!(controller.is_paused());
        eprintln!("native quiet smoke: manual Pause stable with quiet ON");
        controller.resume();
        let resume_clock = Instant::now();
        assert!(
            wait_native_condition(&mut controller, Duration::from_secs(6), |controller| {
                native_recorded_end(controller) >= paused_end + u64::from(rate) / 4
            }),
            "quiet recording did not continue after Resume"
        );
        let resumed_frames = native_recorded_end(&controller) - paused_end;
        let resume_elapsed = resume_clock.elapsed().as_secs_f64();
        assert!(
            resumed_frames as f64 <= (resume_elapsed + 0.3) * f64::from(rate),
            "Resume backfilled the manually paused interval"
        );
        assert_eq!(pinned.snapshots[0].frame_range(), pinned_range);
        assert_eq!(unsafe { WaitForSingleObject(handle.0, 0) }, WAIT_TIMEOUT);
        eprintln!("native quiet smoke: Resume advances without paused backfill");
        println!(
            "{}",
            serde_json::json!({
                "scope": "isolated silent Process Loopback; memory snapshots only",
                "sample_rate": rate, "quiet_on_frames": on_end - on_start,
                "quiet_on_seconds_observed": on_elapsed, "quiet_off_end": off_end,
                "paused_end": paused_end, "resumed_frames": resumed_frames,
                "resume_seconds_observed": resume_elapsed,
            })
        );
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
        assert!(
            returned_while_busy,
            "quiet retention waited for the capture mutex"
        );
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
            capture
                .push_native_interleaved_with_markers(0, &vec![0.25, -0.5].repeat(6), false, &[1])
                .unwrap();
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
            capture
                .push_native_interleaved_with_markers(600_000_000, &[0.0; 20], false, &[2, 7])
                .unwrap();
        }
        let live = controller.snapshots(None).unwrap();
        assert!(
            live.iter()
                .all(|snapshot| snapshot.frame_range() == (0..16))
        );
        assert_eq!(&live[0].snapshot().samples[..6], &[0.25; 6]);
        assert_eq!(&live[1].snapshot().samples[..6], &[-0.5; 6]);
        assert_eq!(&live[0].snapshot().samples[6..], &[0.0; 10]);
        assert_eq!(&live[1].snapshot().samples[6..], &[0.0; 10]);
        assert_eq!(edit.generation, 7);
        assert_eq!(edit.bar_markers, [1]);
        assert_eq!(edit.snapshots[0].snapshot().samples, [0.25; 6]);
        assert_eq!(edit.snapshots[1].snapshot().samples, [-0.5; 6]);

        controller.set_retain_silence(false);
        controller.request_snapshots(None).recv().unwrap();
        {
            let mut session = lock(&controller.state);
            assert!(!session.retain_silence);
            assert_eq!(session.generation, 7);
            session
                .capture
                .as_mut()
                .unwrap()
                .push_native_interleaved_with_markers(1_600_000_000, &[0.0; 200], false, &[])
                .unwrap();
        }
        let after = controller.snapshots(None).unwrap();
        assert!(after[0].frame_range().end < 116);
        assert!(after[0].frame_range().end >= 16);
        assert_eq!(after[0].frame_range(), after[1].frame_range());
        assert_eq!(
            &after[0].snapshot().samples[..16],
            &live[0].snapshot().samples
        );
        assert_eq!(
            &after[1].snapshot().samples[..16],
            &live[1].snapshot().samples
        );
    }

    #[test]
    fn quiet_retention_retries_a_failed_requested_policy_without_another_ui_change() {
        let mut session = CaptureSession::new();
        session.paused = false;
        session.next_retry = Instant::now() + Duration::from_secs(3600);
        session.requested_retain_silence = true;
        session.mode_error = Some("temporary source-control timeout".into());
        session.next_mode_retry = Instant::now();
        let mut capture = SynchronizedCapture::new(10, 1, None, DurationChoice::Seconds20).unwrap();
        capture
            .push_native_interleaved(0, &[0.25; 4], false)
            .unwrap();
        session.capture = Some(capture);

        session.poll();

        assert!(session.retain_silence);
        assert!(session.requested_retain_silence);
        assert!(session.mode_error.is_none());
        assert!(!session.paused);
        let capture = session.capture.as_ref().unwrap();
        assert!(capture.retain_silence());
        assert_eq!(capture.capture().frame_range(), 0..4);
        assert_eq!(
            capture
                .capture()
                .track_snapshot_range(0, 0..4)
                .unwrap()
                .samples,
            [0.25; 4]
        );
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
        assert!(
            returned_while_busy,
            "snapshot request waited for capture work"
        );
        let snapshots = result.unwrap().recv().unwrap().unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].frame_range(), 1..3);
        assert_eq!(snapshots[0].snapshot().samples, [0.2, 0.3]);
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
        assert!(
            edit.snapshots
                .iter()
                .all(|snapshot| snapshot.frame_range() == (0..200))
        );

        controller.set_retention(DurationChoice::Seconds10);
        let live = controller.snapshots(None).unwrap();
        assert_eq!(live[0].frame_range(), 100..200);
        assert_eq!(edit.bar_markers, [10, 30, 170, 190]);
        assert_eq!(edit.snapshots[0].frame_range(), 0..200);
        assert_eq!(edit.snapshots[0].snapshot().samples, vec![0.25; 200]);
        assert_eq!(edit.snapshots[1].snapshot().samples, vec![-0.5; 200]);
    }

    #[test]
    fn edit_snapshots_share_the_capture_pages_and_copy_only_the_one_being_written() {
        let controller = Controller::new().unwrap();
        {
            let mut session = lock(&controller.state);
            session.sources[1] = Some(SourceSettings::default());
            session.shape = Some((10, 1, Some(1)));
            let mut capture =
                SynchronizedCapture::new(10, 1, Some(1), DurationChoice::Seconds20).unwrap();
            let input: Vec<_> = (1..=95)
                .flat_map(|frame| [frame as f32, -(frame as f32)])
                .collect();
            capture
                .push_native_interleaved_with_markers(0, &input, false, &[])
                .unwrap();
            session.capture = Some(capture);
        }
        let first = controller.request_edit_snapshot().recv().unwrap().unwrap();
        let second = controller.request_edit_snapshot().recv().unwrap().unwrap();
        // 95 frames at 10 Hz are nine complete one-second pages and one still open.
        let view = first.snapshots[0].capture();
        assert_eq!(first.snapshots[0].frame_range(), 0..95);
        assert_eq!(view.page_count(), 10);
        // Both lanes read one view, and a second freeze reuses every complete page.
        assert_eq!(view.shared_pages(first.snapshots[1].capture()), 10);
        assert_eq!(view.shared_pages(second.snapshots[0].capture()), 9);
        {
            let session = lock(&controller.state);
            let live = session.capture.as_ref().unwrap().capture().freeze();
            assert_eq!(
                view.shared_pages(&live),
                9,
                "the edit copied the capture's pages"
            );
        }
        // A selection pins only the pages it touches, at its absolute frames.
        let selected = controller.snapshots(Some(28..41)).unwrap();
        assert_eq!(selected[0].frame_range(), 28..41);
        assert_eq!(selected[0].capture().page_count(), 3);
        assert_eq!(selected[0].capture().shared_pages(view), 3);
        assert_eq!(selected[1].snapshot().samples[0], -29.0);
        let expected: Vec<f32> = (1..=95).map(|frame| frame as f32).collect();
        assert_eq!(first.snapshots[0].snapshot().samples, expected);
        assert_eq!(second.snapshots[1].snapshot().samples[94], -95.0);
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
        assert_eq!(snapshots[0].snapshot().samples[0], 201.0);
        assert_eq!(snapshots[1].snapshot().samples[0], -201.0);
        let selected = controller.snapshots(Some(298..302)).unwrap();
        assert_eq!(selected[0].frame_range(), 298..302);
        assert_eq!(selected[0].snapshot().samples, [299.0, 300.0, 301.0, 302.0]);
        assert_eq!(
            selected[1].snapshot().samples,
            [-299.0, -300.0, -301.0, -302.0]
        );

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

    fn waiting(change: impl FnOnce(&mut WorkerWait)) -> Option<Duration> {
        let mut state = WorkerWait {
            paused: false,
            sender_connected: false,
            since_sender_packet: None,
            mode_retry_in: None,
            source_retry_in: None,
        };
        change(&mut state);
        worker_wait(&state)
    }

    #[test]
    fn a_paused_or_fully_connected_worker_blocks_until_it_is_woken() {
        assert_eq!(waiting(|_| {}), None);
        assert_eq!(
            waiting(|state| {
                state.paused = true;
                state.sender_connected = true;
                state.mode_retry_in = Some(Duration::ZERO);
                state.source_retry_in = Some(Duration::ZERO);
            }),
            None,
        );
    }

    #[test]
    fn the_worker_wakes_for_the_earliest_retry_it_owes() {
        let source = Duration::from_millis(1_500);
        let mode = Duration::from_millis(300);
        assert_eq!(
            waiting(|state| state.source_retry_in = Some(source)),
            Some(source)
        );
        assert_eq!(
            waiting(|state| {
                state.source_retry_in = Some(source);
                state.mode_retry_in = Some(mode);
            }),
            Some(mode),
        );
        assert_eq!(
            waiting(|state| state.mode_retry_in = Some(Duration::ZERO)),
            Some(Duration::ZERO)
        );
    }

    #[test]
    fn a_sender_source_is_read_quickly_only_while_its_packets_keep_arriving() {
        let sender = |since: Option<u64>| {
            waiting(|state| {
                state.sender_connected = true;
                state.since_sender_packet = since.map(Duration::from_millis);
                state.source_retry_in = Some(Duration::from_secs(2));
            })
        };
        assert_eq!(sender(None), Some(SENDER_IDLE_POLL));
        assert_eq!(sender(Some(0)), Some(SENDER_ACTIVE_POLL));
        assert_eq!(sender(Some(249)), Some(SENDER_ACTIVE_POLL));
        assert_eq!(sender(Some(250)), Some(SENDER_IDLE_POLL));
        // A lost connection stops the socket timer; reconnecting is the source retry.
        assert_eq!(
            waiting(|state| {
                state.since_sender_packet = Some(Duration::ZERO);
                state.source_retry_in = Some(Duration::from_secs(2));
            }),
            Some(Duration::from_secs(2)),
        );
    }

    #[test]
    fn a_missing_source_is_one_that_is_neither_open_nor_being_opened() {
        let mut session = CaptureSession::new();
        session.paused = false;
        session.next_retry = Instant::now() + Duration::from_secs(1);
        assert!(session.source_missing());
        let wait = session.next_wait(Instant::now()).unwrap();
        assert!(wait <= Duration::from_secs(1) && wait > Duration::from_millis(500));
        let (_sender, result) = mpsc::sync_channel(1);
        session.pending.push(PendingInput {
            source: CaptureSource::System,
            result,
        });
        assert!(!session.source_missing());
        // The open thread wakes the worker itself when it finishes.
        assert_eq!(session.next_wait(Instant::now()), None);
    }

    #[test]
    fn repaint_requests_go_out_at_once_then_once_per_interval_and_keep_the_last_change() {
        let start = Instant::now();
        let at = |milliseconds| start + Duration::from_millis(milliseconds);
        let mut throttle = RepaintThrottle::default();
        assert_eq!(throttle.update(false, at(0)), Notify::Idle);
        assert_eq!(throttle.update(true, at(0)), Notify::Now);
        // One second of a stream that delivers a packet every 10 ms.
        let mut requests = 0;
        for tick in 1..=100 {
            match throttle.update(true, at(tick * 10)) {
                Notify::Now => requests += 1,
                Notify::Later(delay) => assert!(!delay.is_zero() && delay < UI_NOTIFY_INTERVAL),
                Notify::Idle => panic!("a change was dropped"),
            }
        }
        assert_eq!(
            requests, 12,
            "no more repaints than the old 80 ms refresh timer"
        );
        // The stream stops. Its last change is still delivered, once.
        assert_eq!(
            throttle.update(false, at(1_000)),
            Notify::Later(Duration::from_millis(40))
        );
        assert_eq!(throttle.update(false, at(1_040)), Notify::Now);
        assert_eq!(throttle.update(false, at(1_041)), Notify::Idle);
        assert_eq!(throttle.update(true, at(9_000)), Notify::Now);
    }

    #[test]
    fn the_display_stamp_separates_status_changes_from_waveform_and_meter_data() {
        let mut session = CaptureSession::new();
        let idle = DisplayStamp::of(&session);
        assert!(!DisplayStamp::of(&session).differs(&idle, true));

        session.revision += 1;
        let data = DisplayStamp::of(&session);
        assert!(data.differs(&idle, true));
        assert!(!data.differs(&idle, false), "nothing displays the waveform");

        let mut meters = data.clone();
        meters.meters[0] = Some([-120, -200, -230]);
        assert!(meters.differs(&data, true));
        assert!(!meters.differs(&data, false));

        session.statuses[1] = Some("device lost".into());
        let status = DisplayStamp::of(&session);
        assert!(status.differs(&data, false));
        session.statuses[1] = None;
        assert!(!DisplayStamp::of(&session).differs(&data, false));
        session.error = Some("no source".into());
        assert!(DisplayStamp::of(&session).differs(&data, false));
        session.error = None;
        session.generation += 1;
        assert!(DisplayStamp::of(&session).differs(&data, false));
    }

    #[test]
    fn a_wake_reaches_the_blocked_worker_which_asks_for_a_repaint_only_on_a_change() {
        let controller = Controller::new().unwrap();
        let (requested, repaints) = mpsc::channel();
        controller.set_repaint(Arc::new(move || {
            let _ = requested.send(());
        }));
        let wake = Arc::clone(&lock(&controller.state).wake);
        let long = Duration::from_secs(5);
        let brief = Duration::from_millis(300);

        // The session is paused, so the worker is blocked without a timeout.
        lock(&controller.state).statuses[0] = Some("first".into());
        wake();
        assert!(repaints.recv_timeout(long).is_ok());
        wake();
        assert!(repaints.recv_timeout(brief).is_err(), "nothing changed");

        // Two changes in quick succession: the second one is held back, not lost.
        lock(&controller.state).statuses[0] = Some("second".into());
        wake();
        assert!(repaints.recv_timeout(long).is_ok());
        lock(&controller.state).statuses[0] = Some("third".into());
        wake();
        assert!(repaints.recv_timeout(long).is_ok());
        assert!(repaints.recv_timeout(brief).is_err());

        // While nothing displays the waveform, new audio alone asks for nothing.
        controller.set_watching(false);
        lock(&controller.state).revision += 1;
        wake();
        assert!(repaints.recv_timeout(brief).is_err());
        controller.set_watching(true);
        wake();
        assert!(repaints.recv_timeout(long).is_ok());
    }
}
