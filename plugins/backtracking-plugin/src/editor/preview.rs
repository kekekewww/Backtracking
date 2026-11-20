//! Background peak lookups for a pinned selection view. No capture/session state is
//! retained here: closing the editor may detach a finishing lookup safely.

use std::io;
use std::ops::Range;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};

use backtracking_core::{FrozenTrack, WaveformView, waveform};

use crate::session::{WaveformPeaks, waveform_views};
use crate::settings::WaveformLayout;
use crate::surround::{WideLayout, WideRow};

#[derive(Clone, Debug, PartialEq)]
pub(super) struct PreviewKey {
    pub(super) epoch: u64,
    pub(super) editing_token: u64,
    pub(super) range: Range<u64>,
    pub(super) layout: WaveformLayout,
    pub(super) bins: usize,
}

pub(super) struct PreviewRequest {
    pub(super) wide: Option<crate::surround::WideLayout>,
    pub(super) key: PreviewKey,
    pub(super) snapshots: Arc<[FrozenTrack]>,
}

pub(super) struct PreviewResult {
    pub(super) key: PreviewKey,
    pub(super) peaks: [WaveformPeaks; 2],
}

/// The rows on screen of a pinned view of a capture wider than stereo.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct RowsKey {
    pub(super) epoch: u64,
    pub(super) editing_token: u64,
    pub(super) range: Range<u64>,
    pub(super) rows: Vec<WideRow>,
    pub(super) bins: usize,
}

pub(super) struct RowsRequest {
    pub(super) wide: WideLayout,
    pub(super) key: RowsKey,
    pub(super) snapshots: Arc<[FrozenTrack]>,
}

pub(super) struct RowsResult {
    pub(super) key: RowsKey,
    /// One entry per row of the key, in its order.
    pub(super) peaks: Vec<WaveformPeaks>,
}

/// A request that owns the audio it is drawn from.
pub(super) trait Material {
    fn into_snapshots(self) -> Arc<[FrozenTrack]>;
}

impl Material for PreviewRequest {
    fn into_snapshots(self) -> Arc<[FrozenTrack]> {
        self.snapshots
    }
}

impl Material for RowsRequest {
    fn into_snapshots(self) -> Arc<[FrozenTrack]> {
        self.snapshots
    }
}

/// Renders only the newest of its requests, on a thread of its own.
pub(super) struct LatestWorker<Q, R> {
    pending: Arc<Mutex<PendingRequests<Q>>>,
    result: Arc<Mutex<Option<R>>>,
    wake: mpsc::SyncSender<()>,
    _thread: JoinHandle<()>,
}

pub(super) type PreviewWorker = LatestWorker<PreviewRequest, PreviewResult>;
pub(super) type RowsWorker = LatestWorker<RowsRequest, RowsResult>;

struct PendingRequests<Q> {
    latest: Option<Q>,
    // A replaced request can own the last reference to an old edit's audio pages.
    // The caller only transfers ownership; the worker releases these outside locks.
    retired: Vec<Arc<[FrozenTrack]>>,
}

impl PreviewWorker {
    pub(super) fn start() -> io::Result<Self> {
        Self::start_with_renderer(render_preview)
    }
}

impl RowsWorker {
    pub(super) fn start() -> io::Result<Self> {
        Self::start_with_renderer(render_rows)
    }
}

impl<Q: Material + Send + 'static, R: Send + 'static> LatestWorker<Q, R> {
    fn start_with_renderer(mut render: impl FnMut(Q) -> R + Send + 'static) -> io::Result<Self> {
        let pending = Arc::new(Mutex::new(PendingRequests {
            latest: None,
            retired: Vec::new(),
        }));
        let result = Arc::new(Mutex::new(None));
        let (wake, woken) = mpsc::sync_channel(1);
        let requests = Arc::clone(&pending);
        let results = Arc::clone(&result);
        let worker = thread::Builder::new().name("backtracking-preview".into()).spawn(move || {
            while woken.recv().is_ok() {
                let (request, retired) = {
                    let mut slot = requests.lock().unwrap();
                    (slot.latest.take(), std::mem::take(&mut slot.retired))
                };
                drop(retired);
                let Some(mut request) = request else { continue };
                loop {
                    // Rendering may take a while. Neither slot lock is held here.
                    let output = render(request);
                    let mut slot = requests.lock().unwrap();
                    let retired = std::mem::take(&mut slot.retired);
                    if let Some(next) = slot.latest.take() {
                        drop(slot);
                        drop(retired);
                        request = next;
                        continue;
                    }
                    // Commit only if this is still the newest request. Both locks
                    // are held only for swaps; old peak buffers are freed afterward.
                    let previous = results.lock().unwrap().replace(output);
                    drop(slot);
                    drop(retired);
                    drop(previous);
                    break;
                }
            }
        })?;
        Ok(Self { pending, result, wake, _thread: worker })
    }

    pub(super) fn request(&self, request: Q) {
        let mut slot = self.pending.lock().unwrap();
        if let Some(previous) = slot.latest.replace(request) {
            let previous = previous.into_snapshots();
            if !slot
                .retired
                .iter()
                .any(|snapshots| Arc::ptr_eq(snapshots, &previous))
            {
                slot.retired.push(previous);
            }
        }
        drop(slot);
        let _ = self.wake.try_send(());
    }

    pub(super) fn try_take(&self) -> Option<R> {
        self.result.lock().unwrap().take()
    }
}

/// Reads the rows of a request from its frozen tracks: the pair and the channels of the first,
/// and the sidechain merged from the second.
fn render_rows(request: RowsRequest) -> RowsResult {
    let RowsRequest {
        key,
        snapshots,
        wide,
    } = request;
    let bins = key.bins.clamp(1, waveform::MAX_WAVEFORM_BINS);
    let mut peaks = vec![
        WaveformPeaks {
            minimum: vec![0.0; bins],
            maximum: vec![0.0; bins],
        };
        key.rows.len()
    ];
    let Some(first) = snapshots.first() else {
        return RowsResult { key, peaks };
    };
    if key.range.is_empty()
        || snapshots.iter().take(2).any(|snapshot| {
            let available = snapshot.frame_range();
            key.range.start < available.start || key.range.end > available.end
        })
    {
        return RowsResult { key, peaks };
    }
    for (row, trace) in key.rows.iter().zip(&mut peaks) {
        let (minimum, maximum) = (&mut trace.minimum, &mut trace.maximum);
        match *row {
            WideRow::Pair(side) => {
                first.pair_waveform(&wide.group, side, key.range.clone(), minimum, maximum);
            }
            WideRow::Channel(channel) => {
                first.channel_waveform(channel, key.range.clone(), minimum, maximum);
            }
            WideRow::Sidechain => {
                if let Some(side) = snapshots.get(1) {
                    side.waveform(WaveformView::Merged, key.range.clone(), minimum, maximum);
                }
            }
        }
    }
    RowsResult { key, peaks }
}

fn render_preview(request: PreviewRequest) -> PreviewResult {
    let PreviewRequest {
        key,
        snapshots,
        wide,
    } = request;
    let bins = key.bins.clamp(1, waveform::MAX_WAVEFORM_BINS);
    let mut peaks = std::array::from_fn(|_| WaveformPeaks {
        minimum: vec![0.0; bins],
        maximum: vec![0.0; bins],
    });
    let Some(first) = snapshots.first() else { return PreviewResult { key, peaks } };
    if key.range.is_empty() || snapshots.iter().take(2).any(|snapshot| {
        let available = snapshot.frame_range();
        key.range.start < available.start || key.range.end > available.end
    }) {
        return PreviewResult { key, peaks };
    }
    let dual = snapshots.len() > 1;
    for (index, peaks) in peaks.iter_mut().enumerate() {
        let route = if dual {
            snapshots.get(index).map(|snapshot| (snapshot, WaveformView::Merged))
        } else {
            waveform_views(key.layout, first.channels()).get(index).map(|&view| (first, view))
        };
        let Some((track, view)) = route else { continue };
        // Whole peak blocks come from the capture's peak cache. Only the partial
        // blocks at the ends of a bin are read sample by sample.
        if track.channels() > 2
            && let Some(wide) = &wide
        {
            wide.peaks(view, peaks, |group, side, lo, hi| {
                track.pair_waveform(group, side, key.range.clone(), lo, hi);
            });
        } else {
            track.waveform(
                view,
                key.range.clone(),
                &mut peaks.minimum,
                &mut peaks.maximum,
            );
        }
    }
    PreviewResult { key, peaks }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn request(epoch: u64, layout: WaveformLayout, bins: usize) -> PreviewRequest {
        PreviewRequest {
            wide: None,
            key: PreviewKey {
                epoch,
                editing_token: epoch * 10,
                range: 102..106,
                layout,
                bins,
            },
            snapshots: vec![
                FrozenTrack::from_interleaved(
                    48_000,
                    2,
                    100,
                    &(0..8)
                        .flat_map(|frame| [frame as f32 / 10.0, -(frame as f32) / 20.0])
                        .collect::<Vec<_>>(),
                )
                .unwrap(),
            ]
            .into(),
        }
    }

    #[test]
    fn live_and_frozen_pairs_include_the_rear_height_channel() {
        use crate::{
            session::{Session, Worker},
            settings::SettingsStore,
            surround::speaker_labels,
        };
        let labels = speaker_labels(&[0, 1, 2, 3, 4, 5, 9, 10, 12, 14, 15, 17]);
        let names = labels.iter().map(String::as_str).collect::<Vec<_>>();
        let expected = 0.5 * backtracking_core::spatial::stereo_weights(&names)[11][1];
        let session = Arc::new(Session::new(SettingsStore::default()));
        let mut worker = Worker::with_labels(session.clone(), 48000, 12, 0, 512, labels).unwrap();
        worker.stop_background_for_test();
        let mut audio = vec![vec![0.; 512]; 12];
        audio[11].fill(0.5);
        worker.push(&audio.iter_mut().map(Vec::as_mut_slice).collect::<Vec<_>>());
        let tracks = session.export_snapshots(true).unwrap().tracks;
        let mut live = std::array::from_fn(|_| WaveformPeaks::default());
        let timeline =
            session.waveform_in_range_with_bins(WaveformLayout::Merged, None, 8, &mut live);
        let wide = timeline.wide.unwrap();
        let range = tracks[0].frame_range();
        let mut request = request(1, WaveformLayout::Merged, 8);
        request.key.range = range.clone();
        request.wide = Some(wide.clone());
        request.snapshots = tracks.clone();
        let frozen = render_preview(request);
        for pair in [&live, &frozen.peaks] {
            assert_eq!(pair[0].maximum, [0.; 8]);
            assert_eq!(pair[1].maximum, [expected; 8]);
        }
        let lane = backtracking_core::waveform::PreviewLane {
            track: &tracks[0],
            group: Some(&wide.group),
            selected: None,
            route: backtracking_core::ChannelRoute::Merged,
        };
        let preview = backtracking_core::Preview::with_channels(
            &[lane],
            range,
            None,
            false,
            backtracking_core::Fades::NONE,
        )
        .unwrap();
        preview.render_with(512, |_, sample| assert_eq!(sample, [0., expected]));
    }

    #[test]
    fn preview_scans_original_absolute_frames_for_left_right_and_dual_tracks() {
        let stereo = render_preview(request(4, WaveformLayout::Stereo, 2));
        assert_eq!(stereo.key.range, 102..106);
        assert_eq!(stereo.peaks[0].minimum, [0.2, 0.4]);
        assert_eq!(stereo.peaks[0].maximum, [0.3, 0.5]);
        assert_eq!(stereo.peaks[1].minimum, [-0.15, -0.25]);
        assert_eq!(stereo.peaks[1].maximum, [-0.1, -0.2]);

        let merged = render_preview(request(5, WaveformLayout::Merged, 2));
        assert_eq!(merged.peaks[0].minimum, [0.05, 0.1]);
        assert_eq!(merged.peaks[0].maximum, [0.075, 0.125]);
        assert!(merged.peaks[1].maximum.iter().all(|&value| value == 0.0));

        let mut dual = request(6, WaveformLayout::Stereo, 2);
        dual.snapshots = vec![dual.snapshots[0].clone(), FrozenTrack::from_interleaved(
            48_000,
            1,
            100,
            &(0..8).map(|frame| -(frame as f32) / 10.0).collect::<Vec<_>>(),
        ).unwrap()].into();
        let dual = render_preview(dual);
        assert_eq!(dual.peaks[0].minimum, [0.05, 0.1]);
        assert_eq!(dual.peaks[1].minimum, [-0.3, -0.5]);
        assert_eq!(dual.peaks[1].maximum, [-0.2, -0.4]);
    }

    #[test]
    fn preview_worker_coalesces_rapid_requests_without_locking_during_a_scan() {
        let (started, starting) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let (rendered, rendering) = mpsc::channel();
        let worker = PreviewWorker::start_with_renderer(move |request| {
            if request.key.epoch == 1 {
                started.send(()).unwrap();
                released.recv().unwrap();
            }
            rendered.send(request.key.epoch).unwrap();
            render_preview(request)
        }).unwrap();
        worker.request(request(1, WaveformLayout::Stereo, 2));
        starting.recv_timeout(Duration::from_secs(2)).unwrap();
        for epoch in 2..=200 {
            worker.request(request(epoch, WaveformLayout::Merged, 4));
        }
        assert_eq!(worker.pending.lock().unwrap().latest.as_ref().unwrap().key.epoch, 200);
        release.send(()).unwrap();
        assert_eq!(rendering.recv_timeout(Duration::from_secs(2)).unwrap(), 1);
        assert_eq!(rendering.recv_timeout(Duration::from_secs(2)).unwrap(), 200);
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let latest = loop {
            if let Some(result) = worker.try_take() { break result; }
            assert!(std::time::Instant::now() < deadline, "Latest preview never arrived");
            thread::yield_now();
        };
        assert_eq!(latest.key.epoch, 200);
        assert_eq!(latest.peaks[0].minimum.len(), 4);
        assert!(rendering.try_recv().is_err(), "Intermediate views should be coalesced");
    }

    #[test]
    fn replaced_snapshot_owners_are_released_only_after_worker_progress() {
        let (started, starting) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let (rendered, rendering) = mpsc::channel();
        let worker = PreviewWorker::start_with_renderer(move |request| {
            if request.key.epoch == 1 {
                started.send(()).unwrap();
                released.recv().unwrap();
            }
            rendered.send(request.key.epoch).unwrap();
            render_preview(request)
        }).unwrap();
        worker.request(request(1, WaveformLayout::Merged, 2));
        starting.recv_timeout(Duration::from_secs(2)).unwrap();
        let old = request(2, WaveformLayout::Stereo, 2);
        let old_owner = Arc::downgrade(&old.snapshots);
        worker.request(old);
        worker.request(request(3, WaveformLayout::Merged, 4));
        let alive_while_blocked = old_owner.upgrade().is_some();
        release.send(()).unwrap();
        assert_eq!(rendering.recv_timeout(Duration::from_secs(2)).unwrap(), 1);
        assert_eq!(rendering.recv_timeout(Duration::from_secs(2)).unwrap(), 3);
        assert!(alive_while_blocked,
            "Replacing a queued view must not free the last owner of an edit's audio on the UI thread");
        assert!(old_owner.upgrade().is_none(), "The worker must retire replaced material after its scan");
    }

    #[test]
    fn dropping_preview_worker_does_not_wait_for_a_scan_to_finish() {
        let (started, starting) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let worker = PreviewWorker::start_with_renderer(move |request| {
            started.send(()).unwrap();
            released.recv().unwrap();
            render_preview(request)
        }).unwrap();
        worker.request(request(1, WaveformLayout::Merged, 1));
        starting.recv_timeout(Duration::from_secs(2)).unwrap();
        let (dropped, dropping) = mpsc::channel();
        let drop_thread = thread::spawn(move || { drop(worker); dropped.send(()).unwrap(); });
        let returned = dropping.recv_timeout(Duration::from_millis(500)).is_ok();
        release.send(()).unwrap();
        drop_thread.join().unwrap();
        assert!(returned, "Closing the editor must not join an in-progress preview");
    }
}
