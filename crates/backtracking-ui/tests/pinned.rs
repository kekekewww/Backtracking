//! Pins what the shared painters and widgets draw for the desktop window's parameters and for
//! the plug-in editor's.
//!
//! The files in `golden/` were captured from the two applications' own painters before that
//! code moved into this crate, so a change here that would alter either application fails.
//! Coordinates and colours are compared exactly.

use std::ops::Range;

use backtracking_core::waveform;
use backtracking_ui::{
    LoneSample, Spectrum, SpectrumStop, TraceStyle, TransportGlyph, icon_button,
    paint_colored_waveform_trace, paint_transport_glyph, paint_waveform_trace, status_led,
    trace_bin,
};
use egui::{Color32, Pos2, Rect, Stroke, Vec2};

/// One line per painted shape, with every coordinate written out in full.
fn summary(shapes: &[egui::epaint::ClippedShape]) -> String {
    let point = |p: Pos2| format!("({:?},{:?})", p.x, p.y);
    let points = |ps: &[Pos2]| ps.iter().map(|p| point(*p)).collect::<Vec<_>>().join(" ");
    shapes.iter().map(|clipped| {
        let body = match &clipped.shape {
            egui::Shape::Mesh(mesh) => {
                let mut colors: Vec<_> = mesh.vertices.iter().map(|v| v.color.to_array()).collect();
                colors.dedup();
                format!("mesh fill={colors:?} indices={:?} at {}", mesh.indices,
                    points(&mesh.vertices.iter().map(|v| v.pos).collect::<Vec<_>>()))
            }
            egui::Shape::Path(path) => {
                let color = match &path.stroke.color {
                    egui::epaint::ColorMode::Solid(color) => format!("{:?}", color.to_array()),
                    // What the callback gives at each point and halfway to the next.
                    egui::epaint::ColorMode::UV(callback) => {
                        let rect = Rect::from_points(&path.points);
                        let mut at: Vec<_> = path.points.windows(2)
                            .flat_map(|pair| [pair[0], pair[0].lerp(pair[1], 0.5)]).collect();
                        at.extend(path.points.last());
                        format!("callback{:?}", at.iter().map(|&p| callback(rect, p).to_array()).collect::<Vec<_>>())
                    }
                };
                format!("path closed={} fill={:?} stroke={:?}/{color} at {}", path.closed,
                    path.fill.to_array(), path.stroke.width, points(&path.points))
            }
            egui::Shape::Circle(circle) => format!("circle {} r={:?} fill={:?} stroke={:?}/{:?}",
                point(circle.center), circle.radius, circle.fill.to_array(),
                circle.stroke.width, circle.stroke.color.to_array()),
            egui::Shape::LineSegment { points: ends, stroke } => format!("segment {:?}/{:?} at {}",
                stroke.width, stroke.color.to_array(), points(ends)),
            egui::Shape::Rect(rect) => format!("rect {} {} radius={:?} fill={:?} stroke={:?}/{:?}/{:?}",
                point(rect.rect.min), point(rect.rect.max), rect.corner_radius,
                rect.fill.to_array(), rect.stroke.width, rect.stroke.color.to_array(), rect.stroke_kind),
            egui::Shape::Text(text) => format!("text {:?} at {} size={:?}", text.galley.job.text,
                point(text.pos), text.galley.job.sections.first().map(|s| s.format.font_id.clone())),
            other => format!("{other:?}"),
        };
        format!("clip {} {} | {body}\n", point(clipped.clip_rect.min), point(clipped.clip_rect.max))
    }).collect()
}

/// Runs one headless frame and summarises what it painted.
fn painted(screen: Vec2, add_contents: impl FnMut(&mut egui::Ui)) -> String {
    let context = egui::Context::default();
    let mut output = context.run_ui(egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, screen)),
        ..Default::default()
    }, add_contents);
    let shapes = std::mem::take(&mut output.shapes);
    output.textures_delta.clear();
    output.drop_without_applying_deltas();
    summary(&shapes)
}

#[track_caller]
fn assert_matches_golden(actual: &str, golden: &str) {
    let actual: Vec<_> = actual.lines().collect();
    let golden: Vec<_> = golden.lines().collect();
    for (line, (actual, golden)) in actual.iter().zip(&golden).enumerate() {
        assert_eq!(actual, golden, "line {}", line + 1);
    }
    assert_eq!(actual.len(), golden.len(), "number of lines");
}

/// An overview, an overview with a gap, a sample path, a sample with no neighbour, and nothing.
fn waveform_cases() -> Vec<(&'static str, Vec<f32>, Vec<f32>, Range<u64>)> {
    vec![
        ("envelope", vec![-0.8, -0.2, -0.5], vec![0.4, 0.9, 0.6], 100..1_100),
        ("envelope_gap", vec![-0.4, -0.1, f32::NAN, -0.5, -0.2], vec![0.1, 0.5, f32::NAN, 0.2, 0.4], 0..100),
        ("samples", vec![0.75, -0.5, 0.25], vec![0.75, -0.5, 0.25], 100..103),
        ("samples_lone", vec![0.0, 0.5, f32::NAN, -0.5], vec![0.0, 0.5, f32::NAN, -0.5], 0..4),
        ("empty", vec![], vec![], 0..4),
    ]
}

const PLOT: Rect = Rect::from_min_max(Pos2::new(10.0, 20.0), Pos2::new(130.0, 120.0));
const ACCENT: Color32 = Color32::from_rgb(103, 231, 181);
const STROKE: f32 = 1.5;

fn painted_waveforms(paint: impl Fn(&egui::Painter, &waveform::WaveformGeometry)) -> String {
    waveform_cases().into_iter().map(|(name, minimum, maximum, visible)| {
        let geometry = waveform::waveform_geometry(&minimum, &maximum, visible);
        let shapes = painted(Vec2::splat(400.0), |ui| {
            paint(&ui.painter().with_clip_rect(PLOT), &geometry);
        });
        format!("== {name}\n{shapes}")
    }).collect()
}

#[test]
fn the_desktop_waveform_is_painted_as_before() {
    let actual = painted_waveforms(|painter, geometry| {
        let position = |x, value| Pos2::new(
            PLOT.left() + x * PLOT.width(),
            PLOT.center().y - value * PLOT.height() * 0.47,
        );
        paint_waveform_trace(painter, geometry, position, &TraceStyle {
            stroke: Stroke::new(STROKE, ACCENT),
            fill: ACCENT.linear_multiply(0.4),
            lone_sample: LoneSample::Dot(STROKE * 0.55),
        });
    });
    assert_matches_golden(&actual, include_str!("golden/desktop_waveform.txt"));
}

#[test]
fn the_plugin_waveform_is_painted_as_before() {
    let actual = painted_waveforms(|painter, geometry| {
        let middle = PLOT.center().y;
        let extent = PLOT.height() * 0.47;
        let position = |x: f32, value: f32| Pos2::new(PLOT.left() + PLOT.width() * x, middle - value * extent);
        paint_waveform_trace(painter, geometry, position, &TraceStyle {
            stroke: Stroke::new(STROKE, ACCENT),
            fill: Color32::from_rgba_unmultiplied(ACCENT.r(), ACCENT.g(), ACCENT.b(), 96),
            lone_sample: LoneSample::Dash(STROKE.max(1.0)),
        });
    });
    assert_matches_golden(&actual, include_str!("golden/plugin_waveform.txt"));
}

#[test]
fn hand_built_runs_too_short_to_draw_paint_nothing() {
    let column = waveform::EnvelopeColumn { x: 0.5, minimum: -0.5, maximum: 0.5 };
    let geometry = waveform::WaveformGeometry::Envelope(vec![vec![column], vec![]]);
    let shapes = painted(Vec2::splat(400.0), |ui| {
        paint_waveform_trace(ui.painter(), &geometry, |x, value| Pos2::new(x * 100.0, 50.0 - value * 40.0),
            &TraceStyle {
                stroke: Stroke::new(STROKE, ACCENT),
                fill: ACCENT,
                lone_sample: LoneSample::Dot(1.0),
            });
    });
    assert_eq!(shapes, "");
}

/// The desktop window's trace style and the fill it derives from a colour.
fn desktop_style(color: Color32) -> TraceStyle {
    TraceStyle {
        stroke: Stroke::new(STROKE, color),
        fill: color.linear_multiply(0.4),
        lone_sample: LoneSample::Dot(STROKE * 0.55),
    }
}

/// The plug-in editor's trace style and the fill it derives from a colour.
fn plugin_style(color: Color32) -> TraceStyle {
    TraceStyle {
        stroke: Stroke::new(STROKE, color),
        fill: Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 96),
        lone_sample: LoneSample::Dash(STROKE.max(1.0)),
    }
}

fn plot_position(x: f32, value: f32) -> Pos2 {
    Pos2::new(
        PLOT.left() + x * PLOT.width(),
        PLOT.center().y - value * PLOT.height() * 0.47,
    )
}

/// Band energies for each bin of the cases: one band, two bands, none, all six.
fn case_energies(bin: usize) -> Option<[f32; 6]> {
    match bin % 4 {
        0 => Some([0.0, 0.0, 0.0, 0.0, 0.0, 2.0]),
        1 => Some([1.0, 0.0, 0.0, 0.0, 3.0, 0.0]),
        2 => None,
        _ => Some([1.0; 6]),
    }
}

#[test]
fn the_colored_waveform_is_painted_as_pinned() {
    let spectrum = Spectrum::new([
        SpectrumStop {
            position: 0.0,
            color: Spectrum::DEFAULT_LOW,
        },
        SpectrumStop {
            position: 0.5,
            color: Color32::from_rgb(40, 200, 90),
        },
        SpectrumStop {
            position: 1.0,
            color: Spectrum::DEFAULT_HIGH,
        },
    ])
    .unwrap();
    let palette = spectrum.palette(6);
    let mut actual = String::new();
    for (app, style) in [
        ("desktop", desktop_style as fn(Color32) -> TraceStyle),
        ("plugin", plugin_style),
    ] {
        for (name, minimum, maximum, visible) in waveform_cases() {
            let bins = minimum.len();
            let geometry = waveform::waveform_geometry(&minimum, &maximum, visible);
            let color_at =
                |x| case_energies(trace_bin(x, bins)).and_then(|energies| palette.color(&energies));
            let fill_of = |color: Color32| style(color).fill;
            let shapes = painted(Vec2::splat(400.0), |ui| {
                let painter = ui.painter().with_clip_rect(PLOT);
                paint_colored_waveform_trace(
                    &painter,
                    &geometry,
                    plot_position,
                    &style(ACCENT),
                    color_at,
                    fill_of,
                );
            });
            actual += &format!("== {app} {name}\n{shapes}");
        }
    }
    // A selection painted the way the window paints its flat trace: the geometry again under
    // clip rectangles, the selected part with selected colours.
    let (minimum, maximum) = (vec![-0.8, -0.2, -0.5, -0.3], vec![0.4, 0.9, 0.6, 0.2]);
    let geometry = waveform::waveform_geometry(&minimum, &maximum, 0..4_000);
    let color_at = |x| case_energies(trace_bin(x, 4)).and_then(|energies| palette.color(&energies));
    let selected =
        |color: Color32| Color32::from_rgb(255 - color.r(), 255 - color.g(), 255 - color.b());
    let shapes = painted(Vec2::splat(400.0), |ui| {
        let painter = ui.painter().with_clip_rect(PLOT);
        for (range, chosen) in [(0.0..0.3, false), (0.3..0.6, true), (0.6..1.0, false)] {
            let clip = Rect::from_x_y_ranges(
                PLOT.left() + range.start * PLOT.width()..=PLOT.left() + range.end * PLOT.width(),
                PLOT.y_range(),
            );
            let painter = painter.with_clip_rect(clip);
            if chosen {
                paint_colored_waveform_trace(
                    &painter,
                    &geometry,
                    plot_position,
                    &desktop_style(selected(ACCENT)),
                    |x| color_at(x).map(selected),
                    |color| desktop_style(color).fill,
                );
            } else {
                paint_colored_waveform_trace(
                    &painter,
                    &geometry,
                    plot_position,
                    &desktop_style(ACCENT),
                    color_at,
                    |color| desktop_style(color).fill,
                );
            }
        }
    });
    actual += &format!("== desktop selection\n{shapes}");
    assert_matches_golden(&actual, include_str!("golden/colored_waveform.txt"));
}

#[test]
fn a_colored_trace_without_colours_paints_exactly_the_flat_trace() {
    for style in [desktop_style(ACCENT), plugin_style(ACCENT)] {
        let flat = painted_waveforms(|painter, geometry| {
            paint_waveform_trace(painter, geometry, plot_position, &style);
        });
        let colored = painted_waveforms(|painter, geometry| {
            paint_colored_waveform_trace(
                painter,
                geometry,
                plot_position,
                &style,
                |_| None,
                |_| Color32::RED,
            );
        });
        assert_eq!(colored, flat);
    }
    // One colour everywhere is a plain stroke in that colour, not a callback.
    let geometry = waveform::waveform_geometry(&[-0.5, -0.25], &[0.5, 0.25], 0..100);
    let shapes = painted(Vec2::splat(400.0), |ui| {
        paint_colored_waveform_trace(
            ui.painter(),
            &geometry,
            plot_position,
            &desktop_style(ACCENT),
            |_| Some(Color32::RED),
            |color| color.linear_multiply(0.4),
        );
    });
    assert!(!shapes.contains("callback"), "{shapes}");
    assert!(
        shapes.contains(&format!("stroke=1.5/{:?}", Color32::RED.to_array())),
        "{shapes}"
    );
    // Hand-built runs too short to draw paint nothing.
    let column = waveform::EnvelopeColumn {
        x: 0.5,
        minimum: -0.5,
        maximum: 0.5,
    };
    let geometry = waveform::WaveformGeometry::Envelope(vec![vec![column], vec![]]);
    let shapes = painted(Vec2::splat(400.0), |ui| {
        paint_colored_waveform_trace(
            ui.painter(),
            &geometry,
            plot_position,
            &desktop_style(ACCENT),
            |_| Some(Color32::RED),
            |color| color,
        );
    });
    assert_eq!(shapes, "");
}

#[test]
fn trace_bins_follow_envelope_centres_and_sample_positions() {
    for bins in [1, 3, 7, 1_000, 4_096] {
        let values: Vec<f32> = (0..bins).map(|bin| (bin % 5) as f32 / 10.0).collect();
        let lows: Vec<f32> = values.iter().map(|value| value - 0.5).collect();
        let waveform::WaveformGeometry::Envelope(runs) =
            waveform::waveform_geometry(&lows, &values, 0..bins as u64 * 3)
        else {
            panic!()
        };
        let centres = &runs[0][1..runs[0].len() - 1];
        assert_eq!(centres.len(), bins);
        for (bin, column) in centres.iter().enumerate() {
            assert_eq!(trace_bin(column.x, bins), bin);
        }
        let waveform::WaveformGeometry::Samples(runs) =
            waveform::waveform_geometry(&values, &values, 0..bins as u64)
        else {
            panic!()
        };
        for (bin, point) in runs[0].iter().enumerate() {
            assert_eq!(trace_bin(point.x, bins), bin);
        }
        assert_eq!(trace_bin(1.0, bins), bins - 1);
        assert_eq!(trace_bin(-0.5, bins), 0);
    }
}

const DESKTOP_GLYPH: TransportGlyph = TransportGlyph {
    play: [Vec2::new(-3.5, -5.5), Vec2::new(5.0, 0.0), Vec2::new(-3.5, 5.5)],
    pause_offset: 2.4,
    pause_bar: Vec2::new(2.4, 10.0),
};
const PLUGIN_GLYPH: TransportGlyph = TransportGlyph {
    play: [Vec2::new(-4.0, -6.0), Vec2::new(6.0, 0.0), Vec2::new(-4.0, 6.0)],
    pause_offset: 2.7,
    pause_bar: Vec2::new(2.6, 11.0),
};

fn painted_transport_button(size: f32, glyph: &TransportGlyph, paused: bool, enabled: bool) -> String {
    painted(Vec2::new(300.0, 200.0), |ui| {
        ui.horizontal(|ui| {
            let response = ui.add_enabled(enabled, icon_button(size));
            let color = ui.style().interact(&response).fg_stroke.color;
            paint_transport_glyph(ui.painter(), response.rect.center(), paused, color, glyph);
        });
    })
}

#[test]
fn the_plugin_transport_button_is_painted_as_before() {
    let actual: String = [(true, true), (false, true), (true, false), (false, false)]
        .into_iter()
        .map(|(paused, enabled)| format!(
            "== paused={paused} enabled={enabled}\n{}",
            painted_transport_button(25.0, &PLUGIN_GLYPH, paused, enabled),
        ))
        .collect();
    assert_matches_golden(&actual, include_str!("golden/plugin_transport_button.txt"));
}

/// The desktop button is drawn by a method that needs the running recorder, so it could not
/// be captured. These are the coordinates its code wrote out before the move.
#[test]
fn the_desktop_transport_glyph_keeps_its_measurements() {
    let center = Pos2::new(12.0, 12.0);
    let color = Color32::from_gray(215);
    let play = painted(Vec2::splat(100.0), |ui| {
        paint_transport_glyph(ui.painter(), center, true, color, &DESKTOP_GLYPH);
    });
    assert_eq!(play, "clip (0.0,0.0) (100.0,100.0) | path closed=true fill=[215, 215, 215, 255] \
        stroke=0.0/[0, 0, 0, 0] at (8.5,6.5) (17.0,12.0) (8.5,17.5)\n");

    let pause = painted(Vec2::splat(100.0), |ui| {
        paint_transport_glyph(ui.painter(), center, false, color, &DESKTOP_GLYPH);
    });
    let bar = |x: f32| {
        let bar = Rect::from_center_size(center + Vec2::new(x, 0.0), Vec2::new(2.4, 10.0));
        format!("clip (0.0,0.0) (100.0,100.0) | rect ({:?},{:?}) ({:?},{:?}) \
            radius=CornerRadius {{ nw: 1, ne: 1, sw: 1, se: 1 }} fill=[215, 215, 215, 255] \
            stroke=0.0/[0, 0, 0, 0]/Outside\n", bar.min.x, bar.min.y, bar.max.x, bar.max.y)
    };
    assert_eq!(pause, format!("{}{}", bar(-2.4), bar(2.4)));

    // The button itself is 24 points square, one point smaller than the plug-in's.
    let button = painted_transport_button(24.0, &DESKTOP_GLYPH, true, true);
    assert!(button.starts_with("clip (0.0,0.0) (300.0,200.0) | rect (0.0,0.0) (24.0,24.0) "), "{button}");
    assert!(button.ends_with("at (8.5,6.5) (17.0,12.0) (8.5,17.5)\n"), "{button}");
}

#[test]
fn a_status_led_is_a_dot_centred_in_exactly_the_space_it_was_given() {
    // The desktop toolbar's light, then the plug-in's.
    for (size, radius, color) in [
        (Vec2::new(15.0, 24.0), 4.5, Color32::from_rgb(249, 78, 88)),
        (Vec2::splat(16.0), 4.0, Color32::from_rgb(51, 56, 61)),
    ] {
        let mut expected = String::new();
        let shapes = painted(Vec2::new(300.0, 200.0), |ui| {
            let before = ui.cursor().min;
            status_led(ui, size, radius, color, "Recording");
            assert_eq!(ui.min_rect(), Rect::from_min_size(before, size));
            let center = before + size * 0.5;
            expected = format!("clip (0.0,0.0) (300.0,200.0) | circle ({:?},{:?}) r={radius:?} \
                fill={:?} stroke=0.0/[0, 0, 0, 0]\n", center.x, center.y, color.to_array());
        });
        assert_eq!(shapes, expected);
    }
}
