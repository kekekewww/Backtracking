use super::*;
use backtracking_core::FrozenCapture;

pub(super) type RenderedRange = (Range<u64>, Range<u64>, u32, Vec<u64>, u64);
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ChannelWaveform {
    Native(usize),
    // W3-A constructs these rows when the window displays a wide lane.
    #[cfg_attr(not(test), allow(dead_code))]
    Pair(
        backtracking_core::waveform::PairGroup,
        backtracking_core::waveform::PairSide,
    ),
}

type OwnedTrace = (Vec<f32>, Vec<f32>);

#[derive(Clone, PartialEq)]
struct View {
    generation: u64,
    lanes: [Option<usize>; 2],
    channels: Vec<(usize, ChannelWaveform)>,
    views: Vec<(usize, WaveformView)>,
    zoom: Option<Range<u64>>,
    sizes: Vec<(usize, usize)>,
    view_sizes: Vec<(usize, usize)>,
    rate: u32,
}

struct Work {
    view: View,
    capture: FrozenCapture,
    range: RenderedRange,
    result: mpsc::Sender<Option<Rendered>>,
    cancel: Arc<AtomicBool>,
    #[cfg(test)]
    before_render: Option<Arc<dyn Fn() + Send + Sync>>,
}

struct Rendered {
    view: View,
    range: RenderedRange,
    channels: Vec<OwnedTrace>,
    views: Vec<OwnedTrace>,
}

struct Pending {
    view: View,
    result: mpsc::Receiver<Option<Rendered>>,
    cancel: Arc<AtomicBool>,
}

/// Only one request can own frozen pages. A changed view waits for cancellation
/// before pinning another snapshot, so rapid scrolling cannot queue histories.
pub(super) struct ChannelWaveforms {
    requests: mpsc::SyncSender<Work>,
    pending: Option<Pending>,
    #[cfg(test)]
    pub(super) before_render: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl ChannelWaveforms {
    pub(super) fn new(ui: Arc<UiLink>) -> io::Result<Self> {
        let (requests, work) = mpsc::sync_channel::<Work>(1);
        thread::Builder::new()
            .name("channel waveforms".into())
            .spawn(move || {
                while let Ok(work) = work.recv() {
                    let result = work.result.clone();
                    let rendered = work.render();
                    let _ = result.send(rendered);
                    if let Some(repaint) = ui.repaint.get() {
                        repaint();
                    }
                }
            })?;
        Ok(Self {
            requests,
            pending: None,
            #[cfg(test)]
            before_render: None,
        })
    }

    pub(super) fn render(
        &mut self,
        state: &CaptureSession,
        channels: &[(usize, ChannelWaveform)],
        views: &[(usize, WaveformView)],
        zoom: Option<Range<u64>>,
        traces: &mut [WaveformBuffers<'_>],
        view_traces: &mut [WaveformBuffers<'_>],
    ) -> Option<RenderedRange> {
        let rate = state.shape.map_or(COMMON_RATE, |shape| shape.0);
        let view = View {
            generation: state.generation,
            lanes: state.capture_lanes,
            channels: channels.to_vec(),
            views: views.to_vec(),
            zoom: zoom.clone(),
            sizes: traces
                .iter()
                .map(|(low, high)| (low.len(), high.len()))
                .collect(),
            view_sizes: view_traces
                .iter()
                .map(|(low, high)| (low.len(), high.len()))
                .collect(),
            rate,
        };
        if let Some(pending) = &self.pending {
            match pending.result.try_recv() {
                Ok(Some(rendered)) if rendered.view == view => {
                    self.pending = None;
                    for ((low, high), (source_low, source_high)) in
                        traces.iter_mut().zip(rendered.channels)
                    {
                        low.copy_from_slice(&source_low);
                        high.copy_from_slice(&source_high);
                    }
                    for ((low, high), (source_low, source_high)) in
                        view_traces.iter_mut().zip(rendered.views)
                    {
                        low.copy_from_slice(&source_low);
                        high.copy_from_slice(&source_high);
                    }
                    return Some(rendered.range);
                }
                Err(mpsc::TryRecvError::Empty) => {
                    if pending.view != view {
                        pending.cancel.store(true, Ordering::Release);
                    }
                    return None;
                }
                _ => self.pending = None,
            }
        }
        let Some(capture) = state.capture.as_ref().map(SynchronizedCapture::capture) else {
            for (minimum, maximum) in traces {
                minimum.fill(0.0);
                maximum.fill(0.0);
            }
            for (minimum, maximum) in view_traces {
                minimum.fill(0.0);
                maximum.fill(0.0);
            }
            return Some((0..0, 0..0, rate, Vec::new(), state.revision));
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
        let (result, receive) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let request = Work {
            view: view.clone(),
            // Sealed pages share samples and peaks. Only the open page is copied.
            capture: capture.freeze_range(visible.clone())?,
            range: (
                history,
                visible.clone(),
                rate,
                capture.bar_markers(visible),
                state.revision,
            ),
            result,
            cancel: Arc::clone(&cancel),
            #[cfg(test)]
            before_render: self.before_render.clone(),
        };
        if self.requests.try_send(request).is_ok() {
            self.pending = Some(Pending {
                view,
                result: receive,
                cancel,
            });
        }
        None
    }
}

impl Drop for ChannelWaveforms {
    fn drop(&mut self) {
        if let Some(pending) = &self.pending {
            pending.cancel.store(true, Ordering::Release);
        }
        // Closing requests stops the worker after its current channel. Joining
        // here would put the cold-cache delay back on the window's thread.
    }
}

impl Work {
    fn render(self) -> Option<Rendered> {
        #[cfg(test)]
        if let Some(before_render) = &self.before_render {
            before_render();
        }
        let mut channels = Vec::new();
        let mut views = Vec::new();
        for ((lane, channel), &(low_len, high_len)) in
            self.view.channels.iter().zip(&self.view.sizes)
        {
            if self.cancel.load(Ordering::Acquire) {
                return None;
            }
            let (mut low, mut high) = (vec![0.0; low_len], vec![0.0; high_len]);
            if let Some(track) = self
                .view
                .lanes
                .get(*lane)
                .copied()
                .flatten()
                .and_then(|lane| self.capture.track(lane))
            {
                match channel {
                    ChannelWaveform::Native(index) => {
                        track.channel_waveform(*index, self.range.1.clone(), &mut low, &mut high)
                    }
                    ChannelWaveform::Pair(group, side) => {
                        track.pair_waveform(group, *side, self.range.1.clone(), &mut low, &mut high)
                    }
                };
            }
            channels.push((low, high));
        }
        for (&(lane, view), &(low_len, high_len)) in
            self.view.views.iter().zip(&self.view.view_sizes)
        {
            if self.cancel.load(Ordering::Acquire) {
                return None;
            }
            let (mut low, mut high) = (vec![0.0; low_len], vec![0.0; high_len]);
            if let Some(track) = self
                .view
                .lanes
                .get(lane)
                .copied()
                .flatten()
                .and_then(|lane| self.capture.track(lane))
            {
                track.waveform(view, self.range.1.clone(), &mut low, &mut high);
            }
            views.push((low, high));
        }
        Some(Rendered {
            view: self.view,
            range: self.range,
            channels,
            views,
        })
    }
}
