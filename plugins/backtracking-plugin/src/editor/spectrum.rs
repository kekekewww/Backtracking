//! The waveform colored by what it holds from low to high frequencies: "Waveform color: By
//! frequency" on the Appearance page, as on the desktop (`desktop/windows/src/ui/spectrum.rs`,
//! whose behaviour this follows).
//!
//! Each column of a row takes its color from the band energies of the audio it draws
//! (`backtracking_core::waveform::SpectrumBin`) through a gradient: the user's own when they
//! made one, else the default. Every stop first passes `Theme::legible_spectrum`, which puts a
//! stop that would be hard to see on the plot back to the default color at its position.
//!
//! Threads and locks. The audio callback takes no part: it only fills the lock-free queue the
//! capture thread drains, and nothing here reads, locks or allocates on it. Band energies are
//! read only on this module's thread, `backtracking-waveform-colors`, and only from frozen
//! tracks, never from the live capture:
//! - The live view: the editor's thread pins the frames on screen with
//!   [`Session::try_freeze_frames`](crate::session::Session::try_freeze_frames), a `try_lock`
//!   of the session held for a pointer per second of audio and a copy of the page being
//!   written, as the waveform calls next to it are. A busy session skips the pass.
//! - A pinned view while a selection is edited: the edit's own frozen tracks, with no lock.
//!
//! The first read of a long history builds the band cache, about half a second for 10 minutes
//! of 48 kHz stereo. That read takes only the cache's own per-page channel locks, which the
//! capture thread never takes, and builds it two seconds of audio at a time, so a read that
//! is no longer wanted stops within a few milliseconds. Until a column's colors are in it is
//! drawn in the theme color. One read is in flight at a time, and the next waits at least
//! [`MIN_INTERVAL`] and twice as long as the last one took, so a long, wide view cannot keep
//! the thread busy.
//!
//! The thread starts with the first read, stops a read once the editor's window is closed,
//! and ends by itself after [`IDLE_EXIT`] without one. Turning the choice back to "Single
//! color", reopening the editor and dropping it (the host closing the instance) cancel any
//! read and wait for the thread to end. With "Single color" none of this exists: no thread,
//! no pinned audio, no request and no band cache.

use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use backtracking_core::waveform::{
    PairGroup, PairSide, SPECTRUM_BANDS, SpectrumBin, SpectrumRow, WaveformView,
};
use backtracking_core::FrozenTrack;
use backtracking_ui::{
    OPTIONS_CONTROL_OUTLINE, Spectrum, SpectrumPalette, SpectrumStop, Theme, options_choice,
    options_control_width, options_field, options_note,
};
use egui::{Color32, Pos2, Rect, Stroke, Vec2};

use crate::settings::{GradientStop, SessionSettings, WaveformColoring, WaveformGradient};

/// The least time between the end of one read and the start of the next.
const MIN_INTERVAL: Duration = Duration::from_millis(100);
/// How long the thread waits for a read before it ends.
const IDLE_EXIT: Duration = Duration::from_secs(2);
/// How much audio a read analyses before it looks again at whether it is still wanted.
const WARM_SECONDS: u64 = 2;

/// One row the editor draws, by logical lane: 0 is the main input (or the sidechain when only
/// it is captured), 1 the sidechain captured with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ColorRow {
    View(usize, WaveformView),
    Channel(usize, usize),
    Pair(usize, PairSide),
}

impl ColorRow {
    fn lane(self) -> usize {
        match self {
            Self::View(lane, _) | Self::Channel(lane, _) | Self::Pair(lane, _) => lane,
        }
    }
}

/// The band energies of one row, read for `frames` in `bins.len()` equal parts, as the
/// waveform calls bin the same frames.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct RowSpectrum {
    frames: Range<u64>,
    bins: Vec<SpectrumBin>,
}

impl RowSpectrum {
    /// The color of each of `columns` columns spread evenly over `visible`, as the waveform's
    /// bins are: the bin of this read under each column's middle, `None` where the read has
    /// nothing there (frames it did not cover, or silence). When the read covers exactly
    /// these columns, column `i` takes bin `i`.
    pub(super) fn column_colors(
        &self,
        visible: &Range<u64>,
        columns: usize,
        palette: &SpectrumPalette,
    ) -> Vec<Option<Color32>> {
        let color = |bin: &SpectrumBin| bin.as_ref().and_then(|energies| palette.color(energies));
        if self.frames == *visible && self.bins.len() == columns {
            return self.bins.iter().map(color).collect();
        }
        let shown = u128::from(visible.end.saturating_sub(visible.start));
        let read = u128::from(self.frames.end.saturating_sub(self.frames.start));
        let count = self.bins.len() as u128;
        (0..columns as u128)
            .map(|column| {
                if read == 0 || count == 0 {
                    return None;
                }
                let middle =
                    visible.start + ((2 * column + 1) * shown / (2 * columns as u128)) as u64;
                if !self.frames.contains(&middle) {
                    return None;
                }
                let bin = u128::from(middle - self.frames.start) * count / read;
                self.bins.get(bin as usize).and_then(color)
            })
            .collect()
    }
}

/// What one read covers. A read of a pinned view names the edit it was asked for; one of the
/// live view names none.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Request {
    pub(super) epoch: u64,
    pub(super) edit: Option<u64>,
    pub(super) frames: Range<u64>,
    pub(super) bins: usize,
    pub(super) rows: Vec<ColorRow>,
    /// What lane 0's pair rows are made from, when it is wider than stereo.
    pub(super) group: Option<PairGroup>,
}

struct Job {
    request: Request,
    /// The audio to read, by logical lane.
    tracks: Arc<[FrozenTrack]>,
    cancel: Arc<AtomicBool>,
}

struct Done {
    request: Request,
    rows: Vec<(ColorRow, RowSpectrum)>,
    /// Every row was read; a read stopped early is not kept as done.
    complete: bool,
    elapsed: Duration,
}

/// Reads the band energies of every row of `request` that `tracks` has, while `keep_going`
/// says so; the flag is `false` when it stopped early. Pair rows need the request's group;
/// rows of a lane with no audio are left out.
fn read(
    request: &Request,
    tracks: &[FrozenTrack],
    keep_going: impl Fn() -> bool,
) -> (Vec<(ColorRow, RowSpectrum)>, bool) {
    let mut rows = Vec::with_capacity(request.rows.len());
    for &row in &request.rows {
        if !keep_going() {
            return (rows, false);
        }
        let Some(track) = tracks.get(row.lane()) else {
            continue;
        };
        let spectrum_row = match row {
            ColorRow::View(_, view) => SpectrumRow::View(view),
            ColorRow::Channel(_, channel) => SpectrumRow::Channel(channel),
            ColorRow::Pair(_, side) => match &request.group {
                Some(group) => SpectrumRow::Pair(group, side),
                None => continue,
            },
        };
        let held = track.frame_range();
        let start = request.frames.start.clamp(held.start, held.end);
        let frames = start..request.frames.end.clamp(start, held.end);
        // The band cache is built a part at a time, so that a read no longer wanted ends
        // soon. A part already built costs a sum of its blocks.
        let part = u64::from(track.sample_rate().max(1)) * WARM_SECONDS;
        let mut from = frames.start;
        let mut one = [None];
        while from < frames.end {
            if !keep_going() {
                return (rows, false);
            }
            let to = frames.end.min(from + part);
            track.spectrum(spectrum_row, from..to, &mut one);
            from = to;
        }
        let mut bins = vec![None; request.bins];
        track.spectrum(spectrum_row, frames.clone(), &mut bins);
        rows.push((row, RowSpectrum { frames, bins }));
    }
    (rows, true)
}

/// Whether the editor's window is open, so a read is worth finishing.
pub(super) type Showing = Arc<dyn Fn() -> bool + Send + Sync>;

struct Worker {
    jobs: Option<mpsc::SyncSender<Job>>,
    results: mpsc::Receiver<Done>,
    /// Set when the worker is let go of: ends the read in flight.
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    fn start(showing: Showing) -> Option<Self> {
        let (jobs, requests) = mpsc::sync_channel::<Job>(1);
        let (finished, results) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let thread = thread::Builder::new()
            .name("backtracking-waveform-colors".into())
            .spawn(move || {
                while let Ok(job) = requests.recv_timeout(IDLE_EXIT) {
                    let started = Instant::now();
                    let keep_going = || {
                        !job.cancel.load(Ordering::Acquire)
                            && !stopped.load(Ordering::Acquire)
                            && showing()
                    };
                    let (rows, complete) = read(&job.request, &job.tracks, keep_going);
                    let done = Done {
                        request: job.request,
                        rows,
                        complete,
                        elapsed: started.elapsed(),
                    };
                    // The audio is let go of here, off the editor's thread.
                    drop(job.tracks);
                    if finished.send(done).is_err() {
                        break;
                    }
                }
            })
            .ok()?;
        Some(Self {
            jobs: Some(jobs),
            results,
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for Worker {
    /// Ends the read in flight within one part of it and waits for the thread, so no thread
    /// outlives the editor.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        drop(self.jobs.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The colors read for the rows on screen, and the read in flight.
pub(super) struct WaveformColors {
    epoch: Option<u64>,
    rows: Vec<(ColorRow, RowSpectrum)>,
    worker: Option<Worker>,
    pending: Option<(Request, Arc<AtomicBool>)>,
    /// The last read that finished every row.
    done: Option<Request>,
    next_request: Option<Instant>,
    showing: Showing,
}

impl WaveformColors {
    pub(super) fn new(showing: Showing) -> Self {
        Self {
            epoch: None,
            rows: Vec::new(),
            worker: None,
            pending: None,
            done: None,
            next_request: None,
            showing,
        }
    }

    /// The colors last read for `row`.
    pub(super) fn row(&self, row: ColorRow) -> Option<&RowSpectrum> {
        self.rows
            .iter()
            .find(|(source, _)| *source == row)
            .map(|(_, spectrum)| spectrum)
    }

    /// Each of `columns` columns' color for `row` across `visible`, quiet gaps between two
    /// colors filled; `None` for a column to draw in the theme color.
    pub(super) fn row_colors(
        &self,
        row: ColorRow,
        visible: &Range<u64>,
        columns: usize,
        palette: &SpectrumPalette,
    ) -> Vec<Option<Color32>> {
        self.row(row).map_or_else(
            || vec![None; columns],
            |spectrum| {
                let mut colors = spectrum.column_colors(visible, columns, palette);
                fill_quiet_columns(&mut colors);
                colors
            },
        )
    }

    /// A read is in flight.
    #[cfg(test)]
    pub(super) fn busy(&self) -> bool {
        self.pending.is_some()
    }

    /// Whether the thread is running.
    #[cfg(test)]
    pub(super) fn started(&self) -> bool {
        self.worker.is_some()
    }

    /// Cancels any read, waits for the thread to end and lets every color go.
    pub(super) fn stop(&mut self) {
        if let Some((_, cancel)) = &self.pending {
            cancel.store(true, Ordering::Release);
        }
        *self = Self::new(Arc::clone(&self.showing));
    }

    /// Takes the reads that finished. A thread that ended by itself is let go of.
    fn collect(&mut self) {
        let Some(worker) = &self.worker else {
            return;
        };
        let mut ended = false;
        loop {
            let done = match worker.results.try_recv() {
                Ok(done) => done,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    ended = true;
                    break;
                }
            };
            if self
                .pending
                .as_ref()
                .is_some_and(|(pending, _)| *pending == done.request)
            {
                self.pending = None;
            }
            self.next_request = Some(Instant::now() + (done.elapsed * 2).max(MIN_INTERVAL));
            if Some(done.request.epoch) != self.epoch {
                continue;
            }
            for (row, spectrum) in done.rows {
                match self.rows.iter_mut().find(|(source, _)| *source == row) {
                    Some(old) => old.1 = spectrum,
                    None => self.rows.push((row, spectrum)),
                }
            }
            if done.complete {
                self.done = Some(done.request);
            }
        }
        if ended {
            // Any read it was given went with it.
            self.worker = None;
            self.pending = None;
        }
    }

    /// Asks for the colors of `wanted` when they are not in or on their way, using `pin` for
    /// its audio by logical lane. `coloring` other than by frequency stops the thread, drops
    /// every color and reads nothing.
    pub(super) fn update(
        &mut self,
        coloring: WaveformColoring,
        wanted: Option<Request>,
        pin: impl FnOnce(&Request) -> Option<Arc<[FrozenTrack]>>,
    ) {
        if coloring != WaveformColoring::Frequency {
            if self.worker.is_some() || !self.rows.is_empty() || self.epoch.is_some() {
                self.stop();
            }
            return;
        }
        self.collect();
        let Some(wanted) = wanted else {
            return;
        };
        if Some(wanted.epoch) != self.epoch {
            self.epoch = Some(wanted.epoch);
            self.rows.clear();
            self.done = None;
        }
        // Rows no longer on screen keep their colors for when they come back.
        if self.done.as_ref() == Some(&wanted) {
            return;
        }
        if let Some((pending, cancel)) = &self.pending {
            if pending.rows != wanted.rows
                || pending.epoch != wanted.epoch
                || pending.edit != wanted.edit
            {
                cancel.store(true, Ordering::Release);
            }
            return;
        }
        if self.next_request.is_some_and(|next| next > Instant::now()) {
            return;
        }
        let Some(tracks) = pin(&wanted) else {
            // The capture thread had the session; the next pass tries again.
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let mut job = Job {
            request: wanted.clone(),
            tracks,
            cancel: Arc::clone(&cancel),
        };
        // A thread that ended while idle is started again.
        for _ in 0..2 {
            if self.worker.is_none() {
                self.worker = Worker::start(Arc::clone(&self.showing));
            }
            let Some(jobs) = self.worker.as_ref().and_then(|worker| worker.jobs.as_ref()) else {
                return;
            };
            match jobs.try_send(job) {
                Ok(()) => {
                    self.pending = Some((wanted, cancel));
                    return;
                }
                Err(mpsc::TrySendError::Full(_)) => return,
                Err(mpsc::TrySendError::Disconnected(returned)) => {
                    job = returned;
                    self.worker = None;
                }
            }
        }
    }
}

/// The gradient "By frequency" draws with: the user's own or the default, its stops checked
/// for legibility on the plot.
pub(super) fn drawn_spectrum(settings: &SessionSettings) -> Spectrum {
    let chosen = chosen_spectrum(settings);
    Theme::default().legible_spectrum(&chosen, egui::Visuals::dark().panel_fill)
}

/// The user's own gradient, or the default.
fn chosen_spectrum(settings: &SessionSettings) -> Spectrum {
    settings
        .waveform_gradient
        .as_ref()
        .and_then(|gradient| {
            Spectrum::new(gradient.stops().iter().map(|stop| {
                let [r, g, b] = stop.rgb;
                SpectrumStop {
                    position: stop.position,
                    color: Color32::from_rgb(r, g, b),
                }
            }))
            .ok()
        })
        .unwrap_or_default()
}

/// The band colors to draw with, while the waveform is colored by frequency.
pub(super) fn waveform_palette(settings: &SessionSettings) -> Option<SpectrumPalette> {
    (settings.waveform_coloring == WaveformColoring::Frequency)
        .then(|| drawn_spectrum(settings).palette(SPECTRUM_BANDS))
}

/// `spectrum` as the settings keep it.
fn gradient_of(spectrum: &Spectrum) -> Option<WaveformGradient> {
    WaveformGradient::new(spectrum.stops().iter().map(|stop| {
        let [r, g, b, _] = stop.color.to_array();
        GradientStop {
            position: stop.position,
            rgb: [r, g, b],
        }
    }))
}

/// Gives each run of columns without a color that lies between two colored ones (a quiet
/// moment, a fade, a gap between sounds) the color of its nearer neighbour, so the waveform
/// does not flash its theme color there. Runs at either end stay without one: those may be
/// audio not read yet, as at the newest edge of the live view.
fn fill_quiet_columns(colors: &mut [Option<Color32>]) {
    let mut previous: Option<usize> = None;
    for index in 0..colors.len() {
        if colors[index].is_none() {
            continue;
        }
        if let Some(before) = previous
            && index > before + 1
        {
            let middle = (before + index).div_ceil(2);
            let (left, right) = (colors[before], colors[index]);
            for (gap, color) in colors[before + 1..index].iter_mut().enumerate() {
                *color = if before + 1 + gap < middle { left } else { right };
            }
        }
        previous = Some(index);
    }
}

/// How much of a column's color is kept outside the selection while one is on screen.
const UNSELECTED_KEEP: f32 = 0.38;

/// A column's color outside the selection while one is on screen: darker, and halfway to
/// grey, so the selection is what stands out while each part keeps a hint of its frequencies.
/// The desktop's, so both dim alike.
pub(super) fn unselected_color(color: Color32) -> Color32 {
    let [r, g, b, a] = color.to_array();
    let grey = (u16::from(r) * 54 + u16::from(g) * 183 + u16::from(b) * 19) / 256;
    let mix = |channel: u8| {
        let channel = (u16::from(channel) + grey) / 2;
        (f32::from(channel) * UNSELECTED_KEEP).round() as u8
    };
    Color32::from_rgba_premultiplied(
        mix(r),
        mix(g),
        mix(b),
        (f32::from(a) * UNSELECTED_KEEP.max(0.5)).round() as u8,
    )
}

/// The fill under a column of `color`: what the editor fills under its theme color, 96 of
/// 255 opaque, for any color.
pub(super) fn trace_fill(color: Color32) -> Color32 {
    if color.a() == 255 {
        Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 96)
    } else {
        color.gamma_multiply(96.0 / 255.0)
    }
}

/// Paints `spectrum` across `rect`, with a tick at each stop.
fn paint_gradient(painter: &egui::Painter, rect: Rect, spectrum: &Spectrum) {
    const STEPS: usize = 96;
    let mut mesh = egui::Mesh::default();
    for step in 0..=STEPS {
        let position = step as f32 / STEPS as f32;
        let x = rect.left() + position * rect.width();
        let color = spectrum.at(position);
        mesh.colored_vertex(Pos2::new(x, rect.top()), color);
        mesh.colored_vertex(Pos2::new(x, rect.bottom()), color);
        if step > 0 {
            let vertex = (step * 2) as u32;
            mesh.add_triangle(vertex - 2, vertex - 1, vertex);
            mesh.add_triangle(vertex, vertex - 1, vertex + 1);
        }
    }
    painter.add(egui::Shape::mesh(mesh));
    for stop in spectrum.stops() {
        let x = rect.left() + stop.position * rect.width();
        painter.line_segment(
            [
                Pos2::new(x, rect.bottom() - 4.0),
                Pos2::new(x, rect.bottom()),
            ],
            Stroke::new(2.0, Color32::from_gray(235)),
        );
    }
    painter.rect_stroke(
        rect,
        2.0,
        Stroke::new(1.0, OPTIONS_CONTROL_OUTLINE),
        egui::StrokeKind::Inside,
    );
}

/// Where a stop added to `spectrum` goes: the middle of the widest gap between two stops.
fn added_stop(spectrum: &Spectrum) -> SpectrumStop {
    let stops = spectrum.stops();
    let (from, to) = stops
        .windows(2)
        .map(|pair| (pair[0].position, pair[1].position))
        .max_by(|a, b| (a.1 - a.0).total_cmp(&(b.1 - b.0)))
        .unwrap_or((0.0, 1.0));
    let position = (from + to) / 2.0;
    SpectrumStop {
        position,
        color: spectrum.at(position),
    }
}

/// The rows on the Appearance page that color the waveform, in the desktop's words.
pub(super) fn waveform_color_options(ui: &mut egui::Ui, settings: &mut SessionSettings) {
    options_field(ui, "Waveform color", |ui| {
        options_choice(
            ui,
            "waveform-color",
            &mut settings.waveform_coloring,
            WaveformColoring::ALL.map(|coloring| (coloring, coloring.label().to_owned())),
        );
    });
    if settings.waveform_coloring != WaveformColoring::Frequency {
        return;
    }
    let chosen = chosen_spectrum(settings);
    let drawn = drawn_spectrum(settings);
    let mut edited = None;
    let mut reset = false;
    options_field(ui, "Frequency colors", |ui| {
        let width = options_control_width(ui);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 16.0), egui::Sense::hover());
        paint_gradient(ui.painter(), rect, &drawn);
        ui.horizontal(|ui| {
            ui.set_width(width);
            ui.small("Low");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.small("High");
            });
        });
        let mut stops = chosen.stops().to_vec();
        let count = stops.len();
        let mut changed = false;
        let mut removed = None;
        for index in 0..count {
            ui.push_id(("gradient-stop", index), |ui| {
                ui.horizontal(|ui| {
                    let [r, g, b, _] = stops[index].color.to_array();
                    let mut rgb = [r, g, b];
                    if ui.color_edit_button_srgb(&mut rgb).changed() {
                        let [r, g, b] = rgb;
                        stops[index].color = Color32::from_rgb(r, g, b);
                        changed = true;
                    }
                    if index == 0 || index + 1 == count {
                        ui.label(if index == 0 { "Low" } else { "High" });
                        return;
                    }
                    // A middle stop moves between its neighbours, so the list keeps its
                    // order under the pointer.
                    let range =
                        stops[index - 1].position * 100.0..=stops[index + 1].position * 100.0;
                    let mut percent = stops[index].position * 100.0;
                    if ui
                        .add(
                            egui::Slider::new(&mut percent, range)
                                .suffix(" %")
                                .fixed_decimals(0),
                        )
                        .on_hover_text(
                            "Where this color sits between the lowest and the highest frequencies",
                        )
                        .changed()
                    {
                        stops[index].position = (percent / 100.0).clamp(0.0, 1.0);
                        changed = true;
                    }
                    if ui.small_button("Remove").clicked() {
                        removed = Some(index);
                    }
                });
            });
        }
        if let Some(index) = removed {
            stops.remove(index);
            changed = true;
        }
        ui.horizontal(|ui| {
            if ui
                .add_enabled(count < Spectrum::MAX_STOPS, egui::Button::new("Add color"))
                .clicked()
                && let Ok(spectrum) = Spectrum::new(stops.iter().copied())
            {
                stops.push(added_stop(&spectrum));
                changed = true;
            }
            reset = ui
                .add_enabled(
                    settings.waveform_gradient.is_some(),
                    egui::Button::new("Reset"),
                )
                .clicked();
        });
        if changed && let Ok(spectrum) = Spectrum::new(stops) {
            edited = Some(spectrum);
        }
        options_note(
            ui,
            "Each moment of the waveform mixes these colors by how much of its sound is low or high.",
        );
        if drawn != chosen {
            options_note(
                ui,
                "A color too hard to see on the waveform's background is drawn in the default color instead.",
            );
        }
    });
    if reset {
        settings.waveform_gradient = None;
    } else if let Some(spectrum) = edited {
        settings.waveform_gradient = gradient_of(&spectrum);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn tone(frequency: f64, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|frame| {
                (0.5 * (std::f64::consts::TAU * frequency * frame as f64 / f64::from(RATE)).sin())
                    as f32
            })
            .collect()
    }

    const LOW: [f32; 6] = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    const HIGH: [f32; 6] = [0.0, 0.0, 0.0, 0.0, 0.0, 1.0];

    fn always() -> Showing {
        Arc::new(|| true)
    }

    fn request(rows: Vec<ColorRow>, frames: Range<u64>, bins: usize) -> Request {
        Request {
            epoch: 1,
            edit: None,
            frames,
            bins,
            rows,
            group: None,
        }
    }

    fn near(color: Color32, expected: Color32) -> bool {
        color
            .to_array()
            .iter()
            .zip(expected.to_array())
            .map(|(a, b)| u32::from(a.abs_diff(b)))
            .sum::<u32>()
            < 60
    }

    #[test]
    fn each_column_takes_the_bin_under_it_and_no_data_is_no_color() {
        let palette = Spectrum::default().palette(SPECTRUM_BANDS);
        let (low, high) = (Spectrum::DEFAULT_LOW, Spectrum::DEFAULT_HIGH);
        let read = RowSpectrum {
            frames: 1_000..1_400,
            bins: vec![Some(LOW), None, Some(HIGH), Some([0.0; 6])],
        };
        assert_eq!(
            read.column_colors(&(1_000..1_400), 4, &palette),
            [Some(low), None, Some(high), None]
        );
        assert_eq!(
            read.column_colors(&(1_000..1_400), 8, &palette),
            [Some(low), Some(low), None, None, Some(high), Some(high), None, None]
        );
        // The live view moved on: what the read did not cover has no color yet.
        assert_eq!(
            read.column_colors(&(1_200..1_600), 4, &palette),
            [Some(high), None, None, None]
        );
        assert!(read.column_colors(&(1_000..1_400), 0, &palette).is_empty());
        let mut colors = [None, Some(low), None, None, None, None, Some(high), None];
        fill_quiet_columns(&mut colors);
        assert_eq!(
            colors,
            [None, Some(low), Some(low), Some(low), Some(high), Some(high), Some(high), None]
        );
    }

    #[test]
    fn a_read_colors_each_row_from_its_own_audio_and_stops_when_asked() {
        let frames = RATE as usize;
        let audio: Vec<f32> = tone(70.0, frames)
            .into_iter()
            .zip(tone(9_000.0, frames))
            .flat_map(|(left, right)| [left, right])
            .collect();
        let track = FrozenTrack::from_interleaved(RATE, 2, 500, &audio).unwrap();
        let tracks: Arc<[FrozenTrack]> = vec![track].into();
        let rows = vec![
            ColorRow::View(0, WaveformView::Left),
            ColorRow::View(0, WaveformView::Right),
            ColorRow::View(1, WaveformView::Merged),
            ColorRow::Pair(0, PairSide::Left),
        ];
        let asked = request(rows, 0..10_000_000, 8);
        let (read_rows, complete) = read(&asked, &tracks, || true);
        // The sidechain has no audio and a pair row needs its group: both are left out.
        assert!(complete);
        assert_eq!(read_rows.len(), 2);
        let palette = Spectrum::default().palette(SPECTRUM_BANDS);
        for ((row, spectrum), expected) in read_rows
            .iter()
            .zip([Spectrum::DEFAULT_LOW, Spectrum::DEFAULT_HIGH])
        {
            assert_eq!(spectrum.frames, 500..500 + frames as u64, "{row:?}");
            let colors = spectrum.column_colors(&spectrum.frames.clone(), 8, &palette);
            for color in &colors[1..] {
                assert!(near(color.unwrap(), expected), "{row:?}: {color:?}");
            }
        }
        // The same bins as one read of the whole range, whatever the parts were.
        let mut whole = vec![None; 8];
        tracks[0].spectrum(SpectrumRow::View(WaveformView::Left), 500..500 + frames as u64, &mut whole);
        assert_eq!(read_rows[0].1.bins, whole);
        // A read told to stop stops before a row or between its parts.
        let (none, complete) = read(&asked, &tracks, || false);
        assert!(none.is_empty() && !complete);
        let calls = std::cell::Cell::new(0);
        let (partial, complete) = read(&asked, &tracks, || {
            calls.set(calls.get() + 1);
            calls.get() < 4
        });
        assert!(!complete);
        assert_eq!(partial.len(), 1);
    }

    #[test]
    fn a_cancelled_cold_read_of_ten_minutes_ends_soon() {
        // Ten minutes of mono noise whose band cache nothing has built.
        let mut white = 0x2545_f491_u32;
        let audio: Vec<f32> = (0..RATE as usize * 600)
            .map(|_| {
                white ^= white << 13;
                white ^= white >> 17;
                white ^= white << 5;
                (f64::from(white) / f64::from(u32::MAX) - 0.5) as f32
            })
            .collect();
        let tracks: Arc<[FrozenTrack]> =
            vec![FrozenTrack::from_interleaved(RATE, 1, 0, &audio).unwrap()].into();
        drop(audio);
        let asked = request(
            vec![ColorRow::View(0, WaveformView::Merged)],
            0..RATE as u64 * 600,
            1_600,
        );
        let mut colors = WaveformColors::new(always());
        colors.update(WaveformColoring::Frequency, Some(asked), |_| Some(tracks.clone()));
        assert!(colors.started() && colors.busy());
        thread::sleep(Duration::from_millis(5));
        // Back to a single color: the read is cancelled and the thread waited for.
        let stopping = Instant::now();
        colors.update(WaveformColoring::Single, None, |_| panic!("nothing is pinned"));
        let waited = stopping.elapsed();
        assert!(!colors.started() && !colors.busy());
        assert!(waited < Duration::from_millis(250), "waited {waited:?}");
    }

    #[test]
    fn single_color_starts_nothing_and_frequency_reads_then_lets_go() {
        let audio = tone(70.0, RATE as usize * 3);
        let tracks: Arc<[FrozenTrack]> =
            vec![FrozenTrack::from_interleaved(RATE, 1, 0, &audio).unwrap()].into();
        let rows = vec![ColorRow::View(0, WaveformView::Merged)];
        let asked = request(rows.clone(), 0..RATE as u64 * 3, 64);
        let mut colors = WaveformColors::new(always());
        for _ in 0..3 {
            colors.update(WaveformColoring::Single, Some(asked.clone()), |_| {
                panic!("a single color pins no audio")
            });
        }
        assert!(!colors.started() && !colors.busy());

        let pin = |_: &Request| Some(tracks.clone());
        colors.update(WaveformColoring::Frequency, Some(asked.clone()), pin);
        assert!(colors.started() && colors.busy());
        let deadline = Instant::now() + Duration::from_secs(10);
        while colors.busy() {
            assert!(Instant::now() < deadline, "the colors never came");
            thread::sleep(Duration::from_millis(5));
            colors.update(WaveformColoring::Frequency, Some(asked.clone()), pin);
        }
        let read = colors.row(rows[0]).unwrap();
        assert_eq!(read.bins.len(), 64);
        assert!(read.bins[1..].iter().all(Option::is_some));
        // The same view again reads nothing more.
        colors.update(WaveformColoring::Frequency, Some(asked.clone()), |_| {
            panic!("no new read")
        });
        // A new epoch (the capture was cleared) drops the old colors.
        let cleared = Request { epoch: 2, ..asked.clone() };
        colors.update(WaveformColoring::Frequency, Some(cleared), |_| None);
        assert!(colors.row(rows[0]).is_none());
        // Back to a single color lets the thread and every color go.
        colors.update(WaveformColoring::Single, Some(asked), |_| panic!("nothing read"));
        assert!(!colors.started() && colors.row(rows[0]).is_none());
    }

    #[test]
    fn the_thread_ends_when_idle_or_hidden_and_starts_again_when_needed() {
        let audio = tone(9_000.0, RATE as usize);
        let tracks: Arc<[FrozenTrack]> =
            vec![FrozenTrack::from_interleaved(RATE, 1, 0, &audio).unwrap()].into();
        let rows = vec![ColorRow::View(0, WaveformView::Merged)];
        let shown = Arc::new(AtomicBool::new(true));
        let showing = Arc::clone(&shown);
        let mut colors = WaveformColors::new(Arc::new(move || showing.load(Ordering::Acquire)));
        let pin = |_: &Request| Some(tracks.clone());
        let settle = |colors: &mut WaveformColors, asked: &Request| {
            let deadline = Instant::now() + Duration::from_secs(10);
            while colors.busy() {
                assert!(Instant::now() < deadline);
                thread::sleep(Duration::from_millis(5));
                colors.update(WaveformColoring::Frequency, Some(asked.clone()), pin);
            }
        };
        let first = request(rows.clone(), 0..RATE as u64 / 2, 16);
        colors.update(WaveformColoring::Frequency, Some(first.clone()), pin);
        settle(&mut colors, &first);
        assert!(colors.row(rows[0]).is_some());
        // Left without a read, the thread ends by itself.
        thread::sleep(IDLE_EXIT + Duration::from_millis(300));
        colors.update(WaveformColoring::Frequency, Some(first.clone()), pin);
        assert!(!colors.started());
        // The next view starts it again.
        let second = request(rows.clone(), RATE as u64 / 2..RATE as u64, 16);
        colors.update(WaveformColoring::Frequency, Some(second.clone()), pin);
        assert!(colors.started());
        settle(&mut colors, &second);
        assert_eq!(colors.row(rows[0]).unwrap().frames, RATE as u64 / 2..RATE as u64);
        // With the window closed a read stops at once and is not taken as done.
        shown.store(false, Ordering::Release);
        thread::sleep(MIN_INTERVAL);
        let third = request(rows.clone(), 0..RATE as u64, 16);
        colors.update(WaveformColoring::Frequency, Some(third.clone()), pin);
        settle(&mut colors, &third);
        assert_eq!(colors.done.as_ref(), Some(&second));
    }

    #[test]
    fn the_selection_keeps_its_colors_and_the_rest_is_dimmed_toward_grey() {
        for color in [
            Spectrum::DEFAULT_LOW,
            Spectrum::DEFAULT_HIGH,
            Color32::from_rgb(40, 200, 90),
        ] {
            let dimmed = unselected_color(color);
            let plot = backtracking_ui::PLOT_BACKGROUND;
            let keep = u16::from(255 - dimmed.a());
            let [r, g, b] = [0, 1, 2].map(|channel| {
                let over = u16::from(plot.to_array()[channel]) * keep / 255;
                (u16::from(dimmed.to_array()[channel]) + over).min(255) as u8
            });
            let shown = Color32::from_rgb(r, g, b);
            let ratio = |color| backtracking_ui::contrast_ratio(color, plot);
            assert!(ratio(shown) < ratio(color), "{color:?} dims to {shown:?}");
            assert!(ratio(shown) > 1.2, "{color:?} dims to {shown:?}");
        }
        // The fill under an opaque color is the editor's own fill under its theme color.
        let accent = Color32::from_rgb(103, 231, 181);
        assert_eq!(trace_fill(accent), Color32::from_rgba_unmultiplied(103, 231, 181, 96));
        let dimmed = trace_fill(unselected_color(accent));
        assert!(dimmed.a() < 96 && dimmed.a() > 0);
    }

    #[test]
    fn the_users_gradient_is_drawn_unless_a_stop_is_lost_on_the_plot() {
        let mut settings = SessionSettings::default();
        assert_eq!(drawn_spectrum(&settings), Spectrum::default());
        assert!(waveform_palette(&settings).is_none());
        settings.waveform_coloring = WaveformColoring::Frequency;
        assert_eq!(
            waveform_palette(&settings).unwrap().band(0),
            Some(Spectrum::DEFAULT_LOW)
        );
        let green = Color32::from_rgb(40, 200, 90);
        let user = Spectrum::new([
            SpectrumStop { position: 0.0, color: Color32::from_rgb(250, 200, 40) },
            SpectrumStop { position: 0.4, color: Color32::from_rgb(30, 10, 60) },
            SpectrumStop { position: 1.0, color: green },
        ])
        .unwrap();
        settings.waveform_gradient = gradient_of(&user);
        assert_eq!(chosen_spectrum(&settings), user);
        let drawn = drawn_spectrum(&settings);
        // The dark stop is drawn in the default color at its place; the others are kept.
        assert_eq!(drawn.stops()[0].color, Color32::from_rgb(250, 200, 40));
        assert_eq!(drawn.stops()[1].color, Spectrum::default().at(0.4));
        assert_eq!(drawn.stops()[2].color, green);
        let added = added_stop(&user);
        assert_eq!(added.position, 0.7);
        assert_eq!(added.color, user.at(0.7));
    }

    /// Runs the Appearance rows once, with `click` pressed and released at a point.
    fn appearance(settings: &mut SessionSettings, click: Option<Pos2>) -> egui::FullOutput {
        let context = egui::Context::default();
        let run = |events: Vec<egui::Event>, settings: &mut SessionSettings| {
            context.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(560.0, 800.0))),
                    events,
                    ..Default::default()
                },
                |ui| waveform_color_options(ui, settings),
            )
        };
        let mut output = run(Vec::new(), settings);
        if let Some(position) = click {
            output.textures_delta.clear();
            output.drop_without_applying_deltas();
            let press = |pressed| egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            output = run(vec![egui::Event::PointerMoved(position), press(true)], settings);
            output.textures_delta.clear();
            output.drop_without_applying_deltas();
            output = run(vec![press(false)], settings);
        }
        output.textures_delta.clear();
        output
    }

    fn button_at(output: &egui::FullOutput, label: &str) -> Option<Pos2> {
        output.shapes.iter().find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) if text.galley.job.text == label => {
                Some(text.visual_bounding_rect().center())
            }
            _ => None,
        })
    }

    #[test]
    fn the_appearance_rows_add_remove_and_reset_stops_and_show_only_by_frequency() {
        let mut settings = SessionSettings::default();
        let output = appearance(&mut settings, None);
        assert!(button_at(&output, "Add color").is_none());
        output.drop_without_applying_deltas();

        settings.waveform_coloring = WaveformColoring::Frequency;
        let output = appearance(&mut settings, None);
        let add = button_at(&output, "Add color").expect("the gradient editor is shown");
        output.drop_without_applying_deltas();
        appearance(&mut settings, Some(add)).drop_without_applying_deltas();
        let gradient = settings.waveform_gradient.clone().expect("a stop was added");
        assert_eq!(gradient.stops().len(), 3);
        assert_eq!(gradient.stops()[1].position, 0.5);
        let output = appearance(&mut settings, None);
        let remove = button_at(&output, "Remove").expect("a middle stop can be removed");
        output.drop_without_applying_deltas();
        appearance(&mut settings, Some(remove)).drop_without_applying_deltas();
        assert_eq!(settings.waveform_gradient.as_ref().unwrap().stops().len(), 2);
        let output = appearance(&mut settings, None);
        let reset = button_at(&output, "Reset").unwrap();
        output.drop_without_applying_deltas();
        appearance(&mut settings, Some(reset)).drop_without_applying_deltas();
        assert_eq!(settings.waveform_gradient, None);
        assert_eq!(settings.waveform_coloring, WaveformColoring::Frequency);
    }
}
