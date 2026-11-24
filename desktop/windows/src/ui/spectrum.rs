//! The waveform coloured by what it holds from low to high frequencies: "Waveform color: By
//! frequency" on the Appearance page.
//!
//! Each column of a row takes its colour from the band energies of the audio it draws
//! (`backtracking_core::waveform::SpectrumBin`) through a gradient: the user's own when they
//! made one, else the one a chosen look offers (`AddonLook::offered_spectrum`, the
//! `spectrum` of its appearance), else the default. Every stop first passes
//! `Theme::legible_spectrum`, which puts a stop that would be hard to see back to the default
//! colour at its position, as `Theme::legible` puts back a look's colour.
//!
//! Threads and locks. Band energies are read only on this module's thread,
//! `backtracking-waveform-colours`, and only from frozen audio, never from the live capture:
//! - The live view: the window's thread pins the frames on screen with
//!   [`Controller::try_freeze_frames`](crate::controller::Controller::try_freeze_frames), a
//!   `try_lock` of the capture session held for a pointer per second of audio and a copy of
//!   the page being written (what the channel rows' worker holds it for), and hands the pinned
//!   tracks to the thread. A busy session skips the request until a later pass.
//! - The frozen editing copy: the thread reads the edit's own tracks, which need no lock.
//!
//! The first read of a long history builds the band cache, about half a second for 10 minutes
//! of 48 kHz stereo, while the window keeps drawing. That read takes only the cache's own
//! per-page channel locks, which the capture thread never takes. Until a column's colours are
//! in it is drawn in the single colour, and the thread asks the window to draw again when they
//! arrive. One read is in flight at a time, and the next waits twice as long as the last one
//! took, so a long, wide view cannot keep the thread busy.
//!
//! With "Single color" none of this exists: no thread, no request and no band cache.

use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use backtracking_core::FrozenTrack;
use backtracking_core::waveform::{PairGroup, SPECTRUM_BANDS, SpectrumBin, SpectrumRow};
use backtracking_ui::{
    OPTIONS_CONTROL_OUTLINE, Spectrum, SpectrumPalette, SpectrumStop, Theme, options_choice,
    options_control_width, options_field, options_note,
};
use eframe::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};

use super::{AddonLook, Recorder, RowSource, text::Language};
use crate::preferences::{Key, WaveformColoring, serialize_gradient};

/// The band energies of one row, read for `frames` in `bins.len()` equal parts, as the
/// waveform calls bin the same frames.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct RowSpectrum {
    frames: Range<u64>,
    bins: Vec<SpectrumBin>,
}

impl RowSpectrum {
    /// The colour of each of `columns` columns spread evenly over `visible`, as the waveform's
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

/// What one read covers. A read for the live view names the revision it was asked at; one
/// for the frozen editing copy names none.
#[derive(Clone, Debug, PartialEq)]
struct Request {
    generation: u64,
    revision: Option<u64>,
    frames: Range<u64>,
    bins: usize,
    rows: Vec<RowSource>,
    /// By logical lane: what its pair rows are made from.
    pairs: [Option<PairGroup>; 2],
}

struct Job {
    request: Request,
    /// The audio to read, by logical lane.
    tracks: [Option<FrozenTrack>; 2],
    cancel: Arc<AtomicBool>,
}

struct Done {
    request: Request,
    rows: Vec<(RowSource, RowSpectrum)>,
    elapsed: Duration,
}

/// Reads the band energies of every row of `job` that its audio has, stopping between rows
/// once `cancel` is set. Pair rows need their group in `pairs`; rows of a lane with no audio
/// are left out.
fn read(job: &Job) -> Vec<(RowSource, RowSpectrum)> {
    let request = &job.request;
    let mut rows = Vec::with_capacity(request.rows.len());
    for &row in &request.rows {
        if job.cancel.load(Ordering::Acquire) {
            break;
        }
        let Some(track) = job.tracks.get(row.lane()).and_then(Option::as_ref) else {
            continue;
        };
        let spectrum_row = match row {
            RowSource::View(_, view) => SpectrumRow::View(view),
            RowSource::Channel(_, channel) => SpectrumRow::Channel(channel),
            RowSource::Pair(lane, side) => match request.pairs.get(lane) {
                Some(Some(group)) => SpectrumRow::Pair(group, side),
                _ => continue,
            },
        };
        let held = track.frame_range();
        let start = request.frames.start.clamp(held.start, held.end);
        let frames = start..request.frames.end.clamp(start, held.end);
        let mut bins = vec![None; request.bins];
        track.spectrum(spectrum_row, frames.clone(), &mut bins);
        rows.push((row, RowSpectrum { frames, bins }));
    }
    rows
}

struct Worker {
    jobs: mpsc::SyncSender<Job>,
    results: mpsc::Receiver<Done>,
}

impl Worker {
    /// Starts the thread. It asks `ctx` to draw again after each read and ends once the
    /// window lets go of `jobs`, after the read it is doing.
    fn start(ctx: &egui::Context) -> Option<Self> {
        let (jobs, requests) = mpsc::sync_channel::<Job>(1);
        let (finished, results) = mpsc::channel();
        let ctx = ctx.clone();
        thread::Builder::new()
            .name("backtracking-waveform-colours".into())
            .spawn(move || {
                for job in requests {
                    let started = Instant::now();
                    let rows = read(&job);
                    let done = Done {
                        request: job.request,
                        rows,
                        elapsed: started.elapsed(),
                    };
                    // The audio is let go of here, off the window's thread.
                    drop(job.tracks);
                    if finished.send(done).is_err() {
                        break;
                    }
                    ctx.request_repaint_of(egui::ViewportId::ROOT);
                }
            })
            .ok()?;
        Some(Self { jobs, results })
    }
}

/// The colours read for the rows on screen, and the read in flight.
#[derive(Default)]
pub(super) struct WaveformColors {
    generation: u64,
    rows: Vec<(RowSource, RowSpectrum)>,
    worker: Option<Worker>,
    pending: Option<(Request, Arc<AtomicBool>)>,
    /// The last read that finished.
    done: Option<Request>,
    next_request: Option<Instant>,
}

impl WaveformColors {
    /// The colours last read for `row`.
    pub(super) fn row(&self, row: RowSource) -> Option<&RowSpectrum> {
        self.rows
            .iter()
            .find(|(source, _)| *source == row)
            .map(|(_, spectrum)| spectrum)
    }

    /// A read is in flight.
    pub(super) fn busy(&self) -> bool {
        self.pending.is_some()
    }

    /// Whether the thread is running, which it does only while the waveform is coloured by
    /// frequency.
    #[cfg(test)]
    fn started(&self) -> bool {
        self.worker.is_some()
    }

    /// Takes the reads that finished.
    fn collect(&mut self) {
        let Some(worker) = &self.worker else {
            return;
        };
        while let Ok(done) = worker.results.try_recv() {
            if self
                .pending
                .as_ref()
                .is_some_and(|(pending, _)| *pending == done.request)
            {
                self.pending = None;
            }
            self.next_request = Some(Instant::now() + done.elapsed * 2);
            if done.request.generation != self.generation {
                continue;
            }
            for (row, spectrum) in done.rows {
                match self.rows.iter_mut().find(|(source, _)| *source == row) {
                    Some(old) => old.1 = spectrum,
                    None => self.rows.push((row, spectrum)),
                }
            }
            self.done = Some(done.request);
        }
    }

    /// Asks for the colours of `wanted` when they are not in or on their way, using `pin` for
    /// its audio by logical lane. `coloring` other than by frequency stops the thread, drops
    /// every colour and reads nothing.
    fn update(
        &mut self,
        coloring: WaveformColoring,
        wanted: Option<Request>,
        pin: impl FnOnce(&Request) -> Option<[Option<FrozenTrack>; 2]>,
        ctx: &egui::Context,
    ) {
        if coloring != WaveformColoring::Frequency {
            if self.worker.is_some() || !self.rows.is_empty() {
                *self = Self::default();
            }
            return;
        }
        self.collect();
        let Some(wanted) = wanted else {
            return;
        };
        if wanted.generation != self.generation {
            self.generation = wanted.generation;
            self.rows.clear();
            self.done = None;
        }
        // Rows no longer on screen keep their colours for when they come back.
        if self.done.as_ref() == Some(&wanted) {
            return;
        }
        if let Some((pending, cancel)) = &self.pending {
            if pending.rows != wanted.rows || pending.generation != wanted.generation {
                cancel.store(true, Ordering::Release);
            }
            return;
        }
        let now = Instant::now();
        if let Some(next) = self.next_request.filter(|&next| next > now) {
            ctx.request_repaint_after_for(next - now, egui::ViewportId::ROOT);
            return;
        }
        let Some(tracks) = pin(&wanted) else {
            // The capture worker had the session; try again soon.
            ctx.request_repaint_after_for(Duration::from_millis(16), egui::ViewportId::ROOT);
            return;
        };
        if self.worker.is_none() {
            self.worker = Worker::start(ctx);
        }
        let Some(worker) = &self.worker else {
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let job = Job {
            request: wanted.clone(),
            tracks,
            cancel: Arc::clone(&cancel),
        };
        if worker.jobs.try_send(job).is_ok() {
            self.pending = Some((wanted, cancel));
        }
    }
}

/// The gradient "By frequency" draws with, before it is checked for legibility: the user's own
/// when they made one, else the one the chosen look offers while a look is chosen, else the
/// default.
pub(super) fn chosen_spectrum(
    user: Option<&Spectrum>,
    look_chosen: bool,
    offered: Option<&Spectrum>,
) -> Spectrum {
    user.or(offered.filter(|_| look_chosen))
        .cloned()
        .unwrap_or_default()
}

/// The gradient drawn for the `user`'s own gradient, if any, and `look`: the chosen one of
/// those ([`chosen_spectrum`]), its stops checked for legibility on `theme`'s plot, the look
/// the window draws with now.
fn drawn_spectrum(user: Option<&Spectrum>, look: &AddonLook, theme: &Theme) -> Spectrum {
    let chosen = chosen_spectrum(
        user,
        look.chosen().is_some(),
        look.offered_spectrum().as_ref(),
    );
    theme.legible_spectrum(&chosen, egui::Visuals::dark().panel_fill)
}

/// Gives each run of columns without a colour that lies between two coloured ones (a quiet
/// moment, a fade, a gap between sounds) the colour of its nearer neighbour, so the waveform
/// does not flash its single colour there. Runs at either end stay without one: those may be
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

/// How much of a column's colour is kept outside the selection while one is on screen.
const UNSELECTED_KEEP: f32 = 0.38;

/// A column's colour outside the selection while one is on screen, on its row or another:
/// darker, and halfway to grey, so the selection is what stands out while each part keeps a
/// hint of its frequencies.
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

/// A column's colour inside the selection: its own, in full.
pub(super) fn selected_color(color: Color32) -> Color32 {
    color
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

/// What the user did on the "Frequency colors" rows in one pass.
#[derive(Debug, Default, PartialEq)]
struct GradientEdit {
    /// The gradient as the user left it, which becomes their own.
    edited: Option<Spectrum>,
    /// Reset was pressed: the user's own gradient goes.
    reset: bool,
}

/// The "Frequency colors" rows: `drawn` painted as a bar, and a row to edit each stop of
/// `chosen`, the gradient in use before it is checked. Editing any stop of it makes the
/// result the user's own, so editing a look's colours starts from them. `own` is whether the
/// user has their own gradient, which Reset removes. `look` names the add-on whose look offers
/// a gradient, which a note says is where the colours come from, or what Reset goes back to.
fn gradient_rows(
    ui: &mut egui::Ui,
    language: Language,
    chosen: &Spectrum,
    drawn: &Spectrum,
    own: bool,
    look: Option<&str>,
) -> GradientEdit {
    let mut edit = GradientEdit::default();
    options_field(ui, language.gradient_field(), |ui| {
        let width = options_control_width(ui);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 16.0), egui::Sense::hover());
        paint_gradient(ui.painter(), rect, drawn);
        ui.horizontal(|ui| {
            ui.set_width(width);
            ui.small(language.gradient_low());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.small(language.gradient_high());
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
                        ui.label(if index == 0 {
                            language.gradient_low()
                        } else {
                            language.gradient_high()
                        });
                        return;
                    }
                    // A middle stop moves between its neighbours, so the list keeps its
                    // order under the pointer. A position between whole percents (a look's
                    // 1/3, say) is left as it is until the user moves it: clamping it would
                    // round it, which counts as an edit and would make the look's colours
                    // the user's own.
                    let range =
                        stops[index - 1].position * 100.0..=stops[index + 1].position * 100.0;
                    let mut percent = stops[index].position * 100.0;
                    if ui
                        .add(
                            egui::Slider::new(&mut percent, range)
                                .clamping(egui::SliderClamping::Edits)
                                .suffix(" %")
                                .fixed_decimals(0),
                        )
                        .on_hover_text(language.gradient_position())
                        .changed()
                    {
                        stops[index].position = (percent / 100.0).clamp(0.0, 1.0);
                        changed = true;
                    }
                    if ui.small_button(language.gradient_remove()).clicked() {
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
                .add_enabled(
                    count < Spectrum::MAX_STOPS,
                    egui::Button::new(language.gradient_add()),
                )
                .clicked()
                && let Ok(spectrum) = Spectrum::new(stops.iter().copied())
            {
                stops.push(added_stop(&spectrum));
                changed = true;
            }
            edit.reset = ui
                .add_enabled(own, egui::Button::new(language.gradient_reset()))
                .clicked();
        });
        if changed && let Ok(spectrum) = Spectrum::new(stops) {
            edit.edited = Some(spectrum);
        }
        options_note(ui, language.gradient_note());
        if let Some(name) = look {
            options_note(
                ui,
                if own {
                    language.gradient_own_over_look(name)
                } else {
                    language.gradient_from_look(name)
                },
            );
        }
        if drawn != chosen {
            options_note(ui, language.gradient_illegible());
        }
    });
    edit
}

impl Recorder {
    /// The gradient "By frequency" draws with now, its stops checked for legibility on this
    /// window's plot.
    pub(super) fn drawn_spectrum(&self) -> Spectrum {
        drawn_spectrum(self.waveform_gradient.as_ref(), &self.look, &self.theme)
    }

    /// The band colours to draw with, while the waveform is coloured by frequency.
    pub(super) fn waveform_palette(&self) -> Option<SpectrumPalette> {
        (self.waveform_coloring == WaveformColoring::Frequency)
            .then(|| self.drawn_spectrum().palette(SPECTRUM_BANDS))
    }

    /// Each column's colour for `row`, drawn with `columns` columns across the visible frames.
    pub(super) fn row_column_colors(
        &self,
        row: RowSource,
        columns: usize,
        palette: &SpectrumPalette,
    ) -> Vec<Option<Color32>> {
        self.waveform_colors.row(row).map_or_else(
            || vec![None; columns],
            |spectrum| {
                let mut colors = spectrum.column_colors(&self.visible_frames, columns, palette);
                fill_quiet_columns(&mut colors);
                colors
            },
        )
    }

    /// Asks for the colours of the rows drawn now, once per pass. Nothing happens while the
    /// waveform has a single colour.
    pub(super) fn refresh_waveform_colors(&mut self, ctx: &egui::Context) {
        let coloring = self.waveform_coloring;
        let wanted = (coloring == WaveformColoring::Frequency)
            .then(|| {
                let bins = self.peaks.first()?.low.len();
                (bins > 0 && !self.visible_frames.is_empty()).then(|| Request {
                    generation: self.generation,
                    revision: if self.editing.is_some() {
                        None
                    } else {
                        self.live_revision
                    },
                    frames: self.visible_frames.clone(),
                    bins,
                    rows: self.peak_rows.clone(),
                    pairs: [0, 1].map(|lane| self.lane_pair(lane).cloned()),
                })
            })
            .flatten();
        let editing = self.editing.as_ref();
        let controller = &self.controller;
        self.waveform_colors.update(
            coloring,
            wanted,
            |request| match editing {
                Some(edit) => Some([0, 1].map(|lane| edit.track(lane).cloned())),
                None => controller
                    .try_freeze_frames(request.frames.clone())
                    .filter(|(_, _, generation)| *generation == request.generation)
                    .map(|(tracks, ..)| tracks),
            },
            ctx,
        );
    }

    /// Chooses how the waveform is coloured and keeps the choice.
    pub(super) fn set_waveform_coloring(&mut self, coloring: WaveformColoring) {
        if self.waveform_coloring == coloring {
            return;
        }
        self.waveform_coloring = coloring;
        if coloring != WaveformColoring::Frequency {
            // Lets the thread and every colour go.
            self.waveform_colors = WaveformColors::default();
        }
        self.save_preference(Key::WaveformColor, coloring.preference());
    }

    /// Takes `gradient` as the user's own, or none for the default, and keeps it once no
    /// button is held, as the accent is kept.
    pub(super) fn set_waveform_gradient(&mut self, gradient: Option<Spectrum>) {
        self.waveform_gradient = gradient;
    }

    /// Writes the gradient when it changed since it was last written.
    pub(super) fn save_waveform_gradient(&mut self) {
        if self.waveform_gradient != self.saved_waveform_gradient {
            let value = self
                .waveform_gradient
                .as_ref()
                .map(serialize_gradient)
                .unwrap_or_default();
            self.save_preference(Key::WaveformGradient, value);
            self.saved_waveform_gradient = self.waveform_gradient.clone();
        }
    }

    /// The rows on the Appearance page that colour the waveform.
    pub(super) fn waveform_color_settings(&mut self, ui: &mut egui::Ui) {
        let language = Language::current();
        options_field(ui, language.waveform_color_field(), |ui| {
            let mut coloring = self.waveform_coloring;
            if options_choice(
                ui,
                "waveform-color",
                &mut coloring,
                WaveformColoring::ALL
                    .into_iter()
                    .map(|coloring| (coloring, language.waveform_coloring(coloring).to_owned())),
            ) {
                self.set_waveform_coloring(coloring);
                ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
            }
        });
        if self.waveform_coloring != WaveformColoring::Frequency {
            return;
        }
        let chosen = chosen_spectrum(
            self.waveform_gradient.as_ref(),
            self.look.chosen().is_some(),
            self.look.offered_spectrum().as_ref(),
        );
        let drawn = self.drawn_spectrum();
        let look = self.look.spectrum_offered_by();
        let GradientEdit { edited, reset } = gradient_rows(
            ui,
            language,
            &chosen,
            &drawn,
            self.waveform_gradient.is_some(),
            look.as_deref(),
        );
        let repaint = reset || edited.is_some();
        if reset {
            self.set_waveform_gradient(None);
            self.save_waveform_gradient();
        } else if let Some(spectrum) = edited {
            self.set_waveform_gradient(Some(spectrum));
        }
        if repaint {
            ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use backtracking_core::waveform::{PairSide, WaveformView};

    const RATE: u32 = 48_000;

    fn tone(frequency: f64, frames: usize, channels: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|frame| {
                let value = (0.5
                    * (std::f64::consts::TAU * frequency * frame as f64 / f64::from(RATE)).sin())
                    as f32;
                std::iter::repeat_n(value, channels)
            })
            .collect()
    }

    fn row(frames: Range<u64>, bins: Vec<SpectrumBin>) -> RowSpectrum {
        RowSpectrum { frames, bins }
    }

    const LOW: [f32; 6] = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    const HIGH: [f32; 6] = [0.0, 0.0, 0.0, 0.0, 0.0, 1.0];

    #[test]
    fn each_column_takes_the_bin_under_it_and_no_data_is_no_colour() {
        let palette = Spectrum::default().palette(SPECTRUM_BANDS);
        let (low, high) = (Spectrum::DEFAULT_LOW, Spectrum::DEFAULT_HIGH);
        let read = row(
            1_000..1_400,
            vec![Some(LOW), None, Some(HIGH), Some([0.0; 6])],
        );
        // The same frames and columns: bin i colours column i.
        assert_eq!(
            read.column_colors(&(1_000..1_400), 4, &palette),
            [Some(low), None, Some(high), None]
        );
        // Twice as many columns over the same frames: each takes the bin under its middle.
        assert_eq!(
            read.column_colors(&(1_000..1_400), 8, &palette),
            [
                Some(low),
                Some(low),
                None,
                None,
                Some(high),
                Some(high),
                None,
                None
            ]
        );
        // The view moved on: what the read did not cover has no colour yet.
        assert_eq!(
            read.column_colors(&(1_200..1_600), 4, &palette),
            [Some(high), None, None, None]
        );
        assert_eq!(
            row(0..0, Vec::new()).column_colors(&(0..100), 3, &palette),
            [None; 3]
        );
        assert!(read.column_colors(&(1_000..1_400), 0, &palette).is_empty());
    }

    #[test]
    fn quiet_columns_between_colours_take_the_nearer_one_and_the_ends_stay_without() {
        let (a, b) = (Some(Color32::RED), Some(Color32::BLUE));
        let mut colors = [None, a, None, None, None, None, b, None, b, None];
        fill_quiet_columns(&mut colors);
        assert_eq!(colors, [None, a, a, a, b, b, b, b, b, None]);
        let mut nothing = [None; 4];
        fill_quiet_columns(&mut nothing);
        assert_eq!(nothing, [None; 4]);
    }

    fn job(tracks: [Option<FrozenTrack>; 2], rows: Vec<RowSource>, frames: Range<u64>) -> Job {
        Job {
            request: Request {
                generation: 1,
                revision: None,
                frames,
                bins: 8,
                rows,
                pairs: [None, None],
            },
            tracks,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    #[test]
    fn a_read_colours_each_row_from_its_own_audio() {
        let frames = RATE as usize;
        // Left low, right high.
        let audio: Vec<f32> = tone(70.0, frames, 1)
            .into_iter()
            .zip(tone(9_000.0, frames, 1))
            .flat_map(|(left, right)| [left, right])
            .collect();
        let track = FrozenTrack::from_interleaved(RATE, 2, 500, &audio).unwrap();
        let rows = vec![
            RowSource::View(0, WaveformView::Left),
            RowSource::View(0, WaveformView::Right),
            RowSource::View(1, WaveformView::Merged),
            RowSource::Pair(0, PairSide::Left),
        ];
        let read = read(&job([Some(track), None], rows, 0..10_000_000));
        // Lane B has no audio, and a pair row needs its group: both are left out.
        assert_eq!(read.len(), 2);
        let palette = Spectrum::default().palette(SPECTRUM_BANDS);
        for ((source, spectrum), expected) in read
            .iter()
            .zip([Spectrum::DEFAULT_LOW, Spectrum::DEFAULT_HIGH])
        {
            // The read keeps to the frames the audio holds.
            assert_eq!(spectrum.frames, 500..500 + frames as u64, "{source:?}");
            let colors = spectrum.column_colors(&spectrum.frames.clone(), 8, &palette);
            // Past the first block's lead-in every column is (nearly) the band's own colour.
            for color in &colors[1..] {
                let color = color.unwrap();
                let distance: u32 = color
                    .to_array()
                    .iter()
                    .zip(expected.to_array())
                    .map(|(a, b)| u32::from(a.abs_diff(b)))
                    .sum();
                assert!(
                    distance < 60,
                    "{source:?}: {color:?} is not near {expected:?}"
                );
            }
        }
        // A cancelled read stops before its first row.
        let track = FrozenTrack::from_interleaved(RATE, 1, 0, &tone(70.0, 4_800, 1)).unwrap();
        let cancelled = job(
            [Some(track), None],
            vec![RowSource::View(0, WaveformView::Merged)],
            0..4_800,
        );
        cancelled.cancel.store(true, Ordering::Release);
        assert!(super::read(&cancelled).is_empty());
    }

    #[test]
    fn the_users_gradient_wins_then_a_chosen_looks_then_the_default() {
        let user = Spectrum::two(
            Color32::from_rgb(250, 200, 40),
            Color32::from_rgb(40, 200, 90),
        );
        let offered = Spectrum::two(
            Color32::from_rgb(200, 90, 250),
            Color32::from_rgb(90, 220, 250),
        );
        assert_eq!(chosen_spectrum(Some(&user), true, Some(&offered)), user);
        assert_eq!(chosen_spectrum(Some(&user), false, None), user);
        assert_eq!(chosen_spectrum(None, true, Some(&offered)), offered);
        // An offer counts only while its look is chosen.
        assert_eq!(
            chosen_spectrum(None, false, Some(&offered)),
            Spectrum::default()
        );
        assert_eq!(chosen_spectrum(None, true, None), Spectrum::default());
        assert_eq!(chosen_spectrum(None, false, None), Spectrum::default());
    }

    const ACCENT: Color32 = Color32::from_rgb(103, 231, 181);
    const THEME_ID: &str = "com.example.theme";

    /// The offers after the example look sends `line`, as the host decodes it.
    fn offers_from(line: &str) -> Arc<crate::addons::AddonAppearances> {
        use backtracking_addon::protocol::{AddonMessage, decode_addon};
        let AddonMessage::Appearance(appearance) = decode_addon(line.as_bytes()).unwrap() else {
            panic!("not an appearance: {line}");
        };
        Arc::new([(THEME_ID.to_owned(), appearance)].into())
    }

    fn stop(position: f32, [r, g, b]: [u8; 3]) -> SpectrumStop {
        SpectrumStop {
            position,
            color: Color32::from_rgb(r, g, b),
        }
    }

    /// The example add-on's look, with its gradient.
    const WARM: &str = r##"{"type":"appearance","colors":{"waveform":"#67e7b5","plot_background":"#141d2a"},"spectrum":[{"color":"#ff8c3c","position":0},{"color":"#f0dc5a80","position":0.45},{"color":"#6ec8ff","position":1}]}"##;

    #[test]
    fn the_users_gradient_beats_the_looks_which_beats_the_default_end_to_end() {
        let mut look = AddonLook::new(Some(THEME_ID.into()));
        let mut theme = Theme::default();
        let mut take = |look: &mut AddonLook, offers| {
            if let Some(newest) = look.update(offers, ACCENT) {
                theme = newest;
            }
            theme.clone()
        };
        let warm = Spectrum::new([
            stop(0.0, [255, 140, 60]),
            // The alpha an add-on sends is not drawn.
            stop(0.45, [240, 220, 90]),
            stop(1.0, [110, 200, 255]),
        ])
        .unwrap();
        let user = Spectrum::two(
            Color32::from_rgb(250, 200, 40),
            Color32::from_rgb(40, 200, 90),
        );

        // The look offers a gradient and the user has none: the look's, on its own plot.
        let theme = take(&mut look, Some(offers_from(WARM)));
        assert_eq!(theme.plot_background, Color32::from_rgb(20, 29, 42));
        assert_eq!(drawn_spectrum(None, &look, &theme), warm);
        // The user's own wins.
        assert_eq!(drawn_spectrum(Some(&user), &look, &theme), user);

        // A new offer that changes only the gradient changes nothing else, and is drawn.
        let moved = WARM.replace("#6ec8ff", "#a0e0ff");
        assert_eq!(look.update(Some(offers_from(&moved)), ACCENT), None);
        assert_eq!(
            drawn_spectrum(None, &look, &theme).stops()[2].color,
            Color32::from_rgb(160, 224, 255)
        );

        // An offer without one, or a gradient the contract drops: the default.
        let without = r##"{"type":"appearance","colors":{"waveform":"#67e7b5"}}"##;
        let theme = take(&mut look, Some(offers_from(without)));
        assert_eq!(drawn_spectrum(None, &look, &theme), Spectrum::default());
        let backwards = WARM.replace(r#""position":0}"#, r#""position":0.9}"#);
        let theme = take(&mut look, Some(offers_from(&backwards)));
        assert_eq!(drawn_spectrum(None, &look, &theme), Spectrum::default());

        // The look turned off: the default, while the add-on still offers its gradient.
        let theme = take(&mut look, Some(offers_from(WARM)));
        assert_eq!(drawn_spectrum(None, &look, &theme), warm);
        look.choose(None);
        let theme = take(&mut look, None);
        assert_eq!(theme, Theme::default());
        assert_eq!(drawn_spectrum(None, &look, &theme), Spectrum::default());
        assert_eq!(drawn_spectrum(Some(&user), &look, &theme), user);

        // A stop lost on the look's plot is drawn in the default colour there, as the user's.
        look.choose(Some(THEME_ID.into()));
        let dark = WARM.replace("#f0dc5a80", "#18202a");
        let theme = take(&mut look, Some(offers_from(&dark)));
        let drawn = drawn_spectrum(None, &look, &theme);
        assert_eq!(
            drawn.stops().iter().map(|stop| stop.color).collect::<Vec<_>>(),
            [
                Color32::from_rgb(255, 140, 60),
                Spectrum::default().at(0.45),
                Color32::from_rgb(110, 200, 255)
            ]
        );
        let users_dark =
            Spectrum::new([stop(0.0, [250, 200, 40]), stop(1.0, [24, 32, 42])]).unwrap();
        assert_eq!(
            drawn_spectrum(Some(&users_dark), &look, &theme).stops()[1].color,
            Spectrum::default().at(1.0)
        );
    }

    /// Draws the "Frequency colors" rows for three passes on one context, clicking the
    /// button labelled `click` (when given) in the last two, and returns what the rows wrote
    /// and what the user did.
    fn gradient_rows_pass(
        language: Language,
        chosen: &Spectrum,
        own: bool,
        look: Option<&str>,
        click: Option<&str>,
    ) -> (Vec<String>, GradientEdit) {
        let context = egui::Context::default();
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(720.0, 600.0));
        let mut texts = Vec::new();
        let mut edit = GradientEdit::default();
        let mut target = None;
        for pass in 0..3 {
            let mut events = Vec::new();
            if let Some(pos) = target {
                events.push(egui::Event::PointerMoved(pos));
                events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: pass == 1,
                    modifiers: egui::Modifiers::default(),
                });
            }
            let mut output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |ui| edit = gradient_rows(ui, language, chosen, chosen, own, look),
            );
            let shapes = std::mem::take(&mut output.shapes);
            output.drop_without_applying_deltas();
            texts.clear();
            for shape in shapes {
                if let egui::Shape::Text(text) = shape.shape {
                    let label = text.galley.text().to_owned();
                    if click == Some(label.as_str()) {
                        target = Some(text.pos + text.galley.rect.center().to_vec2());
                    }
                    texts.push(label);
                }
            }
        }
        (texts, edit)
    }

    #[test]
    fn the_frequency_colors_say_where_they_come_from_and_editing_starts_from_the_looks() {
        let warm = Spectrum::new([
            stop(0.0, [255, 140, 60]),
            stop(0.45, [240, 220, 90]),
            stop(1.0, [110, 200, 255]),
        ])
        .unwrap();
        let name = "Example appearance";
        for language in [Language::English, Language::TraditionalChinese] {
            let from_look = language.gradient_from_look(name);
            let over_look = language.gradient_own_over_look(name);
            // The look's colours, and none of the user's own.
            let (texts, edit) = gradient_rows_pass(language, &warm, false, Some(name), None);
            assert!(texts.contains(&from_look), "{texts:?}");
            assert!(!texts.contains(&over_look), "{texts:?}");
            assert_eq!(edit, GradientEdit::default());
            // Showing stops between whole percents edits nothing either.
            let thirds = Spectrum::new([
                stop(0.0, [255, 140, 60]),
                stop(1.0 / 3.0, [240, 220, 90]),
                stop(0.725, [120, 230, 160]),
                stop(1.0, [110, 200, 255]),
            ])
            .unwrap();
            let (_, edit) = gradient_rows_pass(language, &thirds, false, Some(name), None);
            assert_eq!(edit, GradientEdit::default());
            // Reset has nothing to remove yet.
            let (_, edit) = gradient_rows_pass(
                language,
                &warm,
                false,
                Some(name),
                Some(language.gradient_reset()),
            );
            assert!(!edit.reset);
            // Adding a colour makes the look's stops, and the new one, the user's own.
            let (_, edit) = gradient_rows_pass(
                language,
                &warm,
                false,
                Some(name),
                Some(language.gradient_add()),
            );
            let own = edit.edited.expect("the user's own gradient");
            assert_eq!(own.stops().len(), 4);
            for stop in warm.stops() {
                assert!(own.stops().contains(stop), "{stop:?} in {own:?}");
            }

            // The user's own over the look's: Reset goes back to the look's.
            let (texts, _) = gradient_rows_pass(language, &own, true, Some(name), None);
            assert!(texts.contains(&over_look), "{texts:?}");
            assert!(!texts.contains(&from_look), "{texts:?}");
            let (_, edit) = gradient_rows_pass(
                language,
                &own,
                true,
                Some(name),
                Some(language.gradient_reset()),
            );
            assert_eq!(
                edit,
                GradientEdit {
                    edited: None,
                    reset: true
                }
            );

            // No look with a gradient: no note about one.
            for own in [false, true] {
                let (texts, _) =
                    gradient_rows_pass(language, &Spectrum::default(), own, None, None);
                assert!(texts.contains(&language.gradient_note().to_owned()), "{texts:?}");
                assert!(!texts.iter().any(|text| text.contains(name)), "{texts:?}");
            }
        }
    }

    #[test]
    fn the_selection_keeps_its_colours_and_the_rest_is_dimmed_toward_grey() {
        for color in [
            Spectrum::DEFAULT_LOW,
            Spectrum::DEFAULT_HIGH,
            Color32::from_rgb(40, 200, 90),
        ] {
            assert_eq!(selected_color(color), color);
            let dimmed = unselected_color(color);
            let plot = backtracking_ui::PLOT_BACKGROUND;
            // What the dimmed colour looks like drawn over the plot (it is premultiplied).
            let keep = u16::from(255 - dimmed.a());
            let [r, g, b] = [0, 1, 2].map(|channel| {
                let over = u16::from(plot.to_array()[channel]) * keep / 255;
                (u16::from(dimmed.to_array()[channel]) + over).min(255) as u8
            });
            let shown = Color32::from_rgb(r, g, b);
            let ratio = |color| backtracking_ui::contrast_ratio(color, plot);
            // Darker than the colour itself, and still a waveform on the plot.
            assert!(ratio(shown) < ratio(color), "{color:?} dims to {shown:?}");
            assert!(
                ratio(shown) > 1.2,
                "{color:?} dims to {shown:?}: {}",
                ratio(shown)
            );
        }
        assert_eq!(unselected_color(Color32::TRANSPARENT), Color32::TRANSPARENT);
    }

    #[test]
    fn an_added_stop_splits_the_widest_gap_in_the_gradients_colour_there() {
        let spectrum = Spectrum::new([
            SpectrumStop {
                position: 0.0,
                color: Color32::RED,
            },
            SpectrumStop {
                position: 0.2,
                color: Color32::GREEN,
            },
            SpectrumStop {
                position: 1.0,
                color: Color32::BLUE,
            },
        ])
        .unwrap();
        let added = added_stop(&spectrum);
        assert_eq!(added.position, 0.6);
        assert_eq!(added.color, spectrum.at(0.6));
    }

    #[test]
    fn single_colour_starts_nothing_and_builds_no_band_cache() {
        use crate::controller::Controller;

        let controller = Controller::new().unwrap();
        controller.load_test_capture(RATE, &tone(70.0, RATE as usize * 3, 2), 2);
        let ctx = egui::Context::default();
        let rows = vec![RowSource::View(0, WaveformView::Merged)];
        let wanted = |revision| Request {
            generation: controller.generation(),
            revision: Some(revision),
            frames: 0..RATE as u64 * 3,
            bins: 64,
            rows: rows.clone(),
            pairs: [None, None],
        };
        // The waveform itself, drawn as the window draws it with a single colour.
        let mut low = vec![0.0; 64];
        let mut high = vec![0.0; 64];
        controller
            .try_render_waveforms(
                &[(0, WaveformView::Merged)],
                None,
                &mut [(&mut low, &mut high)],
            )
            .unwrap();
        let mut colors = WaveformColors::default();
        for revision in 0..3 {
            colors.update(
                WaveformColoring::Single,
                Some(wanted(revision)),
                |_| panic!("a single colour pins no audio"),
                &ctx,
            );
        }
        assert!(!colors.started() && !colors.busy());
        assert_eq!(controller.spectrum_cache_pages(), 0);

        // By frequency, the thread reads the pinned audio and the colours come in.
        let pin = |request: &Request| {
            controller
                .try_freeze_frames(request.frames.clone())
                .map(|(tracks, ..)| tracks)
        };
        colors.update(WaveformColoring::Frequency, Some(wanted(3)), pin, &ctx);
        assert!(colors.started() && colors.busy());
        let deadline = Instant::now() + Duration::from_secs(10);
        while colors.busy() {
            assert!(Instant::now() < deadline, "the colours never came");
            thread::sleep(Duration::from_millis(5));
            colors.update(WaveformColoring::Frequency, Some(wanted(3)), pin, &ctx);
        }
        let read = colors.row(rows[0]).unwrap();
        assert_eq!(read.bins.len(), 64);
        assert!(read.bins[1..].iter().all(Option::is_some));
        assert!(controller.spectrum_cache_pages() > 0);
        // The same view again reads nothing more; back to a single colour lets it all go.
        colors.update(
            WaveformColoring::Frequency,
            Some(wanted(3)),
            |_| panic!("no new read"),
            &ctx,
        );
        colors.update(
            WaveformColoring::Single,
            Some(wanted(4)),
            |_| panic!("nothing read"),
            &ctx,
        );
        assert!(!colors.started() && colors.row(rows[0]).is_none());
    }

    /// What turning colour by frequency on costs over a full 10-minute history. Run with
    /// `cargo test --release --locked --manifest-path desktop/windows/Cargo.toml
    /// ten_minute_colour_cost -- --ignored --nocapture`.
    #[test]
    #[ignore = "release timing probe with 230 MB of audio"]
    fn ten_minute_colour_cost() {
        use backtracking_core::{Capture, DurationChoice};

        let seconds = 600;
        let mut capture = Capture::new(RATE, 2, DurationChoice::Minutes10).unwrap();
        capture.set_retain_silence(true);
        let mut white = 0x2545_f491_u32;
        let second: Vec<f32> = (0..RATE as usize)
            .flat_map(|frame| {
                white ^= white << 13;
                white ^= white >> 17;
                white ^= white << 5;
                let noise = f64::from(white) / f64::from(u32::MAX) - 0.5;
                let time = frame as f64 / f64::from(RATE);
                let low = 0.3 * (std::f64::consts::TAU * 80.0 * time).sin();
                [(low + 0.1 * noise) as f32, (0.2 * noise) as f32]
            })
            .collect();
        for _ in 0..seconds {
            capture.push_interleaved(&second).unwrap();
        }
        let frames = capture.frame_range();
        // The part done under the session lock in the live view.
        let started = Instant::now();
        let frozen = capture.freeze_range(frames.clone()).unwrap();
        println!(
            "pin 10 min stereo (the session lock's share): {:.3} ms",
            started.elapsed().as_secs_f64() * 1e3
        );
        let track = frozen.track(0).unwrap();
        for (phase, rows) in [
            (
                "cold, merged row",
                vec![RowSource::View(0, WaveformView::Merged)],
            ),
            (
                "warm, merged row",
                vec![RowSource::View(0, WaveformView::Merged)],
            ),
            (
                "warm, split rows",
                vec![
                    RowSource::View(0, WaveformView::Left),
                    RowSource::View(0, WaveformView::Right),
                ],
            ),
        ] {
            let mut job = job([Some(track.clone()), None], rows, frames.clone());
            job.request.bins = 1_600;
            let started = Instant::now();
            let read = read(&job);
            println!(
                "colours for 1600 columns over 10 min, {phase}: {:.1} ms",
                started.elapsed().as_secs_f64() * 1e3
            );
            assert!(read.iter().all(|(_, row)| row.bins.len() == 1_600));
        }
    }
}
