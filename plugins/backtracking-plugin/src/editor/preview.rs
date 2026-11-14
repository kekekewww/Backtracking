//! Background peak lookups for a pinned selection view. No capture/session state is
//! retained here: closing the editor may detach a finishing lookup safely.

use std::io;
use std::ops::Range;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};

use backtracking_core::{FrozenTrack, WaveformView, waveform};

use crate::session::{WaveformPeaks, waveform_views};
use crate::settings::WaveformLayout;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct PreviewKey {
    pub(super) epoch: u64,
    pub(super) editing_token: u64,
    pub(super) range: Range<u64>,
    pub(super) layout: WaveformLayout,
    pub(super) bins: usize,
}

pub(super) struct PreviewRequest {
    pub(super) key: PreviewKey,
    pub(super) snapshots: Arc<[FrozenTrack]>,
}

pub(super) struct PreviewResult {
    pub(super) key: PreviewKey,
    pub(super) peaks: [WaveformPeaks; 2],
}

pub(super) struct PreviewWorker {
    pending: Arc<Mutex<PendingRequests>>,
    result: Arc<Mutex<Option<PreviewResult>>>,
    wake: mpsc::SyncSender<()>,
    _thread: JoinHandle<()>,
}

#[derive(Default)]
struct PendingRequests {
    latest: Option<PreviewRequest>,
    // A replaced request can own the last reference to an old edit's audio pages.
    // The caller only transfers ownership; the worker releases these outside locks.
    retired: Vec<Arc<[FrozenTrack]>>,
}

impl PreviewWorker {
    pub(super) fn start() -> io::Result<Self> {
        Self::start_with_renderer(render_preview)
    }

    fn start_with_renderer(
        mut render: impl FnMut(PreviewRequest) -> PreviewResult + Send + 'static,
    ) -> io::Result<Self> {
        let pending = Arc::new(Mutex::new(PendingRequests::default()));
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

    pub(super) fn request(&self, request: PreviewRequest) {
        let mut slot = self.pending.lock().unwrap();
        if let Some(previous) = slot.latest.replace(request) {
            if !slot.retired.iter().any(|snapshots| Arc::ptr_eq(snapshots, &previous.snapshots)) {
                slot.retired.push(previous.snapshots);
            }
        }
        drop(slot);
        let _ = self.wake.try_send(());
    }

    pub(super) fn try_take(&self) -> Option<PreviewResult> {
        self.result.lock().unwrap().take()
    }
}

fn render_preview(request: PreviewRequest) -> PreviewResult {
    let PreviewRequest { key, snapshots } = request;
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
        track.waveform(view, key.range.clone(), &mut peaks.minimum, &mut peaks.maximum);
    }
    PreviewResult { key, peaks }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn request(epoch: u64, layout: WaveformLayout, bins: usize) -> PreviewRequest {
        PreviewRequest {
            key: PreviewKey { epoch, editing_token: epoch * 10, range: 102..106, layout, bins },
            snapshots: vec![FrozenTrack::from_interleaved(
                48_000,
                2,
                100,
                &(0..8).flat_map(|frame| [frame as f32 / 10.0, -(frame as f32) / 20.0]).collect::<Vec<_>>(),
            ).unwrap()].into(),
        }
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
