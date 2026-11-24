use std::sync::Arc;

use backtracking_core::waveform::WaveformGeometry;
use egui::epaint::{ColorMode, PathStroke};
use egui::{Color32, Pos2, Stroke, Vec2};

/// The mark for a sample that has no neighbour to connect to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LoneSample {
    /// A filled dot of this radius.
    Dot(f32),
    /// A horizontal dash of this length, starting at the sample.
    Dash(f32),
}

/// How one waveform trace is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TraceStyle {
    /// The sample path and both outlines of an envelope.
    pub stroke: Stroke,
    /// The strip between the outlines of an envelope.
    pub fill: Color32,
    pub lone_sample: LoneSample,
}

/// Paints display geometry from `backtracking_core::waveform::waveform_geometry`.
///
/// `position` places a point of the geometry on screen. It is given the horizontal fraction
/// of the visible range and the signed amplitude, in that order. An envelope becomes one
/// filled strip with an outline above and below; samples become one path per run. Separate
/// runs are never connected.
pub fn paint_waveform_trace(
    painter: &egui::Painter,
    geometry: &WaveformGeometry,
    position: impl Fn(f32, f32) -> Pos2,
    style: &TraceStyle,
) {
    let stroke = style.stroke;
    match geometry {
        WaveformGeometry::Empty => {}
        WaveformGeometry::Envelope(runs) => {
            for run in runs {
                let mut mesh = egui::Mesh::default();
                let mut upper = Vec::with_capacity(run.len());
                let mut lower = Vec::with_capacity(run.len());
                for (index, column) in run.iter().enumerate() {
                    let top = position(column.x, column.maximum);
                    let bottom = position(column.x, column.minimum);
                    mesh.colored_vertex(top, style.fill);
                    mesh.colored_vertex(bottom, style.fill);
                    upper.push(top);
                    lower.push(bottom);
                    if index > 0 {
                        let vertex = (index * 2) as u32;
                        mesh.add_triangle(vertex - 2, vertex - 1, vertex);
                        mesh.add_triangle(vertex, vertex - 1, vertex + 1);
                    }
                }
                if !mesh.indices.is_empty() {
                    painter.add(egui::Shape::mesh(mesh));
                }
                if upper.len() > 1 {
                    painter.add(egui::Shape::line(upper, stroke));
                    painter.add(egui::Shape::line(lower, stroke));
                }
            }
        }
        WaveformGeometry::Samples(runs) => {
            for run in runs {
                let points: Vec<_> = run.iter().map(|point| position(point.x, point.value)).collect();
                if points.len() > 1 {
                    painter.add(egui::Shape::line(points, stroke));
                } else if let Some(&point) = points.first() {
                    match style.lone_sample {
                        LoneSample::Dot(radius) => {
                            painter.circle_filled(point, radius, stroke.color);
                        }
                        LoneSample::Dash(length) => {
                            painter.line_segment([point, point + Vec2::new(length, 0.0)], stroke);
                        }
                    }
                }
            }
        }
    }
}

/// The bin, of `bins`, that a fraction given to the `color_at` of
/// [`paint_colored_waveform_trace`] lies in, for geometry built from `bins` bins: an
/// envelope's bin centre and a sample's own position both map to their bin.
pub fn trace_bin(x: f32, bins: usize) -> usize {
    ((x * bins as f32 + 0.25).max(0.0) as usize).min(bins.saturating_sub(1))
}

/// Paints the same geometry as [`paint_waveform_trace`], with a colour per envelope column and
/// per sample.
///
/// `color_at` is asked for the colour at a horizontal fraction of the visible range: each
/// envelope bin's centre, and each sample's position. [`trace_bin`] turns either into an index
/// of the bins the geometry was built from. The two columns that close an envelope run repeat
/// their neighbour's colour. Where `color_at` answers `None` the trace keeps `style`'s stroke
/// and fill colours; elsewhere the outlines, sample paths and lone samples take the colour it
/// gives and the fill takes `fill_of` of it, for example `|color| theme.waveform_fill(color)`.
/// Widths and the lone-sample mark always come from `style`. Colours change smoothly between
/// neighbouring columns. A `color_at` that is always `None` paints exactly what
/// `paint_waveform_trace` does.
///
/// To draw a selected range differently, paint the geometry again from a painter clipped to
/// that range, with a `color_at` and `style` that give the selected look, the way the flat
/// trace is painted today.
///
/// The fill strip colours each vertex. egui 0.37 has no per-point stroke colour: a path's
/// colour is one colour or a callback from a position on screen (`ColorMode::UV`). So each
/// outline is still one path, with its joins, and its callback interpolates the colours of
/// the two points either side of the screen x it is asked about.
pub fn paint_colored_waveform_trace(
    painter: &egui::Painter,
    geometry: &WaveformGeometry,
    position: impl Fn(f32, f32) -> Pos2,
    style: &TraceStyle,
    color_at: impl Fn(f32) -> Option<Color32>,
    fill_of: impl Fn(Color32) -> Color32,
) {
    let stroke_of = |color: Option<Color32>| color.unwrap_or(style.stroke.color);
    match geometry {
        WaveformGeometry::Empty => {}
        WaveformGeometry::Envelope(runs) => {
            for run in runs {
                let last = run.len().saturating_sub(1);
                let colors: Vec<_> = (0..run.len())
                    .map(|index| {
                        let index = if last >= 2 {
                            index.clamp(1, last - 1)
                        } else {
                            index
                        };
                        color_at(run[index].x)
                    })
                    .collect();
                let mut mesh = egui::Mesh::default();
                let mut upper = Vec::with_capacity(run.len());
                let mut lower = Vec::with_capacity(run.len());
                for (index, (column, color)) in run.iter().zip(&colors).enumerate() {
                    let top = position(column.x, column.maximum);
                    let bottom = position(column.x, column.minimum);
                    let fill = color.map_or(style.fill, &fill_of);
                    mesh.colored_vertex(top, fill);
                    mesh.colored_vertex(bottom, fill);
                    upper.push(top);
                    lower.push(bottom);
                    if index > 0 {
                        let vertex = (index * 2) as u32;
                        mesh.add_triangle(vertex - 2, vertex - 1, vertex);
                        mesh.add_triangle(vertex, vertex - 1, vertex + 1);
                    }
                }
                if !mesh.indices.is_empty() {
                    painter.add(egui::Shape::mesh(mesh));
                }
                if upper.len() > 1 {
                    let strokes: Vec<_> = colors.iter().map(|&color| stroke_of(color)).collect();
                    let stroke = path_stroke(style.stroke.width, &upper, strokes);
                    painter.add(egui::Shape::line(upper, stroke.clone()));
                    painter.add(egui::Shape::line(lower, stroke));
                }
            }
        }
        WaveformGeometry::Samples(runs) => {
            for run in runs {
                let points: Vec<_> = run
                    .iter()
                    .map(|point| position(point.x, point.value))
                    .collect();
                let colors: Vec<_> = run
                    .iter()
                    .map(|point| stroke_of(color_at(point.x)))
                    .collect();
                if points.len() > 1 {
                    let stroke = path_stroke(style.stroke.width, &points, colors);
                    painter.add(egui::Shape::line(points, stroke));
                } else if let (Some(&point), Some(&color)) = (points.first(), colors.first()) {
                    match style.lone_sample {
                        LoneSample::Dot(radius) => {
                            painter.circle_filled(point, radius, color);
                        }
                        LoneSample::Dash(length) => {
                            let stroke = Stroke::new(style.stroke.width, color);
                            painter.line_segment([point, point + Vec2::new(length, 0.0)], stroke);
                        }
                    }
                }
            }
        }
    }
}

/// A path stroke whose colour follows `colors`, one per point of `points`. A single colour
/// is a plain stroke.
fn path_stroke(width: f32, points: &[Pos2], colors: Vec<Color32>) -> PathStroke {
    if colors.windows(2).all(|pair| pair[0] == pair[1]) {
        return PathStroke::new(width, colors.first().copied().unwrap_or_default());
    }
    let mut by_x: Vec<_> = points.iter().map(|point| point.x).zip(colors).collect();
    if !by_x.is_sorted_by(|a, b| a.0 <= b.0) {
        by_x.sort_by(|a, b| a.0.total_cmp(&b.0));
    }
    let by_x: Arc<[(f32, Color32)]> = by_x.into();
    PathStroke {
        width,
        color: ColorMode::UV(Arc::new(move |_, at: Pos2| {
            let next = by_x.partition_point(|&(x, _)| x < at.x);
            match (next.checked_sub(1).map(|index| by_x[index]), by_x.get(next)) {
                (Some((left, from)), Some(&(right, to))) if right > left => {
                    from.lerp_to_gamma(to, (at.x - left) / (right - left))
                }
                (_, Some(&(_, color))) | (Some((_, color)), None) => color,
                (None, None) => Color32::TRANSPARENT,
            }
        })),
        kind: egui::StrokeKind::Middle,
    }
}
