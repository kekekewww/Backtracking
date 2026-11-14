use backtracking_core::waveform::WaveformGeometry;
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
