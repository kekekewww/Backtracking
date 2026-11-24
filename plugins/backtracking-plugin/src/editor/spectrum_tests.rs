//! The editor coloring its waveform by frequency, from audio pushed through the capture as the
//! host's would be, and pictures of it.
//!
//! The pictures are made by `shots`, which is ignored unless asked for:
//! `BACKTRACKING_PLUGIN_SHOTS=<folder> cargo test --locked --manifest-path
//! plugins/backtracking-plugin/Cargo.toml shots -- --ignored --nocapture`. They come from the
//! editor's own drawing code run headless; this file turns what egui would hand the GPU into
//! pixels with a small rasterizer, so they show colors and shapes faithfully but not every
//! detail of the real renderer's anti-aliasing and text.

use std::collections::HashMap;
use std::path::Path;

use backtracking_ui::Spectrum;

use super::*;
use crate::session::Worker;
use crate::settings::{GradientStop, SettingsStore, WaveformGradient};

const RATE: u32 = 48_000;

/// A session that captured `seconds` of `signal(channel, frame)` on `channels` channels, and
/// the worker that keeps it.
fn captured(
    channels: usize,
    labels: Vec<String>,
    seconds: f64,
    signal: impl Fn(usize, usize) -> f32,
) -> (Arc<Session>, Worker) {
    let session = Arc::new(Session::new(SettingsStore::default()));
    let mut worker = Worker::with_labels(session.clone(), RATE, channels, 0, 512, labels).unwrap();
    worker.stop_background_for_test();
    let total = (seconds * f64::from(RATE)) as usize;
    const BLOCK: usize = 4_096;
    for (index, start) in (0..total).step_by(BLOCK).enumerate() {
        let frames = BLOCK.min(total - start);
        let mut audio: Vec<Vec<f32>> = (0..channels)
            .map(|channel| (start..start + frames).map(|frame| signal(channel, frame)).collect())
            .collect();
        worker.push(&audio.iter_mut().map(Vec::as_mut_slice).collect::<Vec<_>>());
        // The capture thread's part: the queue holds a little over two seconds.
        if index % 8 == 7 {
            session.export_snapshots(true);
        }
    }
    session.export_snapshots(true);
    (session, worker)
}

fn sine(frequency: f64, frame: usize) -> f32 {
    (0.6 * (std::f64::consts::TAU * frequency * frame as f64 / f64::from(RATE)).sin()) as f32
}

/// A sweep from 40 Hz to 18 kHz, even in octaves, over `seconds`.
fn sweep(seconds: f64, frame: usize) -> f32 {
    let (low, high) = (40.0_f64, 18_000.0_f64);
    let rate = (high / low).ln() / seconds;
    let time = frame as f64 / f64::from(RATE);
    let phase = std::f64::consts::TAU * low * ((rate * time).exp() - 1.0) / rate;
    (0.6 * phase.sin()) as f32
}

fn noise(frame: usize) -> f32 {
    let mut white = (frame as u32).wrapping_mul(0x9e37_79b9) ^ 0x2545_f491;
    white ^= white << 13;
    white ^= white >> 17;
    white ^= white << 5;
    (f64::from(white) / f64::from(u32::MAX) - 0.5) as f32 * 0.8
}

/// The waveform's place in the pictures and tests, as the editor lays it out at its usual size.
const PLOT: Rect = Rect::from_min_max(Pos2::new(8.0, 8.0), Pos2::new(632.0, 168.0));
const SCREEN: Vec2 = Vec2::new(640.0, 176.0);

/// An editor over `session` drawn with `settings`, its colors read as an open window would
/// read them, once they have all come in.
fn editor(session: &Arc<Session>, settings: &SessionSettings) -> CaptureUi {
    let state = EguiEditorState::from_size(LogicalSize::new(640.0, 212.0), 1.0);
    let mut capture = CaptureUi::new(session.clone(), state);
    capture.waveform_colors = WaveformColors::new(Arc::new(|| true));
    session.update_editor_settings(settings.clone());
    capture.display_size = capture.audio_rect(PLOT, 1.0).size();
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        capture.next_refresh = Instant::now();
        capture.refresh_waveform(false, settings.waveform_layout);
        capture.display_size = capture.audio_rect(PLOT, capture.seconds).size();
        capture.next_refresh = Instant::now();
        capture.refresh_waveform(false, settings.waveform_layout);
        capture.refresh_waveform_colors(settings.waveform_coloring);
        let (rows, _) = capture.color_rows();
        let colored = rows.iter().all(|&row| capture.waveform_colors.row(row).is_some());
        if settings.waveform_coloring == WaveformColoring::Single
            || (colored && !capture.waveform_colors.busy())
        {
            return capture;
        }
        assert!(Instant::now() < deadline, "the colors never came");
        thread::sleep(Duration::from_millis(20));
    }
}

fn frequency() -> SessionSettings {
    SessionSettings {
        waveform_coloring: WaveformColoring::Frequency,
        ..SessionSettings::default()
    }
}

/// What the editor paints of its waveform.
fn drawn(context: &egui::Context, capture: &CaptureUi, settings: &SessionSettings) -> egui::FullOutput {
    context.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
            ..Default::default()
        },
        |ui| capture.draw_waveform(ui, PLOT, capture.seconds, false, settings),
    )
}

/// The fill colors of the waveform meshes, top row first, each as [r, g, b] unmultiplied.
fn fills(output: &egui::FullOutput) -> Vec<Vec<[u8; 3]>> {
    output
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Mesh(mesh) => Some(
                mesh.vertices
                    .iter()
                    .map(|vertex| {
                        let [r, g, b, a] = vertex.color.to_array();
                        let up = |channel: u8| (u16::from(channel) * 255 / u16::from(a.max(1))).min(255) as u8;
                        [up(r), up(g), up(b)]
                    })
                    .collect(),
            ),
            _ => None,
        })
        .collect()
}

#[test]
fn single_color_starts_no_thread_builds_no_band_cache_and_paints_as_before() {
    let (session, _worker) = captured(2, Vec::new(), 3.0, |channel, frame| {
        sine(if channel == 0 { 70.0 } else { 9_000.0 }, frame)
    });
    let single = SessionSettings::default();
    let capture = editor(&session, &single);
    assert!(!capture.waveform_colors.started());
    assert_eq!(session.spectrum_cache_pages(), 0);
    let context = egui::Context::default();
    let before = drawn(&context, &capture, &single);
    // The same picture as a flat trace in the theme color: one fill color, the accent's.
    let accent = accent_color(single.accent);
    let expected = [accent.r(), accent.g(), accent.b()];
    let painted = fills(&before);
    assert!(!painted.is_empty());
    assert!(painted.iter().flatten().all(|&color| color.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 2)));

    // By frequency and back: the thread goes, and the single color paints exactly as before.
    let mut capture = editor(&session, &frequency());
    assert!(session.spectrum_cache_pages() > 0);
    capture.refresh_waveform_colors(WaveformColoring::Single);
    assert!(!capture.waveform_colors.started());
    let after = drawn(&context, &capture, &single);
    assert_eq!(format!("{:?}", before.shapes), format!("{:?}", after.shapes));
    for output in [before, after] {
        output.drop_without_applying_deltas();
    }
}

#[test]
fn by_frequency_colors_each_row_and_dims_outside_a_selection() {
    let (session, _worker) = captured(2, Vec::new(), 3.0, |channel, frame| {
        sine(if channel == 0 { 70.0 } else { 9_000.0 }, frame)
    });
    let settings = SessionSettings {
        waveform_layout: WaveformLayout::Stereo,
        ..frequency()
    };
    let mut capture = editor(&session, &settings);
    assert!(capture.waveform_colors.started());
    let context = egui::Context::default();
    let output = drawn(&context, &capture, &settings);
    let rows = fills(&output);
    assert_eq!(rows.len(), 2);
    let near = |color: [u8; 3], expected: Color32| {
        color.iter().zip(expected.to_array()).map(|(a, b)| u32::from(a.abs_diff(b))).sum::<u32>() < 60
    };
    // Left low, right high: past the first block's lead-in, the default red and blue.
    for (row, expected) in rows.iter().zip([Spectrum::DEFAULT_LOW, Spectrum::DEFAULT_HIGH]) {
        let inner = &row[4..row.len() - 4];
        assert!(inner.iter().all(|&color| near(color, expected)), "{expected:?}: {inner:?}");
    }
    output.drop_without_applying_deltas();

    // A selection: full color inside it, dimmed toward grey outside, on both rows.
    let frames = capture.peak_frames.clone();
    let third = (frames.end - frames.start) / 3;
    capture.selection = Some(frames.start + third..frames.end - third);
    let output = drawn(&context, &capture, &settings);
    let meshes: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Mesh(mesh) => Some((clipped.clip_rect, mesh)),
            _ => None,
        })
        .collect();
    // Three parts a row: before, inside and after the selection.
    assert_eq!(meshes.len(), 6);
    let red = spectrum::trace_fill(Spectrum::DEFAULT_LOW);
    let dimmed = spectrum::trace_fill(spectrum::unselected_color(Spectrum::DEFAULT_LOW));
    let middle = |mesh: &egui::Mesh| mesh.vertices[mesh.vertices.len() / 2].color;
    let close = |a: Color32, b: Color32| {
        a.to_array().iter().zip(b.to_array()).all(|(a, b)| a.abs_diff(b) <= 12)
    };
    assert!(close(middle(meshes[0].1), dimmed), "{:?}", middle(meshes[0].1));
    assert!(close(middle(meshes[1].1), red), "{:?}", middle(meshes[1].1));
    assert!(close(middle(meshes[2].1), dimmed));
    assert!(meshes[1].0.left() > PLOT.left() && meshes[1].0.right() < PLOT.right());
    output.drop_without_applying_deltas();
}

#[test]
fn the_rows_of_a_surround_capture_take_their_own_channels_colors() {
    // 5.1: front left low, front right high, the rest quiet.
    let labels = crate::surround::speaker_labels(&[0, 1, 2, 3, 4, 5]);
    let (session, _worker) = captured(6, labels, 2.0, |channel, frame| match channel {
        0 => sine(70.0, frame),
        1 => sine(9_000.0, frame),
        _ => 0.0,
    });
    let mut capture = editor(&session, &frequency());
    assert!(capture.timeline.wide.is_some());
    let rows = capture.rows.clone();
    let palette = Spectrum::default().palette(backtracking_core::waveform::SPECTRUM_BANDS);
    let bins = capture.row_peaks[0].minimum.len();
    let colors = |capture: &CaptureUi, row: WideRow| {
        let row = wide_color_row(row);
        let rate = capture.timeline.sample_rate;
        capture.waveform_colors.row_colors(row, &capture.peak_frames, bins, &palette, rate)
    };
    let middle = bins / 2;
    for (row, expected) in [
        (WideRow::Pair(PairSide::Left), Some(Spectrum::DEFAULT_LOW)),
        (WideRow::Pair(PairSide::Right), Some(Spectrum::DEFAULT_HIGH)),
        (WideRow::Channel(0), Some(Spectrum::DEFAULT_LOW)),
        (WideRow::Channel(1), Some(Spectrum::DEFAULT_HIGH)),
        // A silent channel has no color: it is drawn in the theme color, as a flat line.
        (WideRow::Channel(2), None),
    ] {
        assert!(rows.contains(&row), "{row:?} is on screen");
        let color = colors(&capture, row)[middle];
        match (color, expected) {
            (Some(color), Some(expected)) => {
                let distance: u32 = color.to_array().iter().zip(expected.to_array())
                    .map(|(a, b)| u32::from(a.abs_diff(b))).sum();
                assert!(distance < 80, "{row:?}: {color:?}");
            }
            (color, expected) => assert_eq!(color, expected, "{row:?}"),
        }
    }
    capture.editor_closed();
    assert!(!capture.waveform_colors.started());
}

/// Turns what egui hands the GPU into pixels: each triangle filled where pixel centres fall,
/// its vertex colors and texture blended over the pixels before it, premultiplied, as egui's
/// own renderers blend.
struct Canvas {
    width: usize,
    height: usize,
    pixels: Vec<[f32; 4]>,
    textures: HashMap<egui::TextureId, egui::ColorImage>,
}

impl Canvas {
    fn new(size: Vec2, pixels_per_point: f32) -> Self {
        let width = (size.x * pixels_per_point).round() as usize;
        let height = (size.y * pixels_per_point).round() as usize;
        Self {
            width,
            height,
            pixels: vec![[0.0, 0.0, 0.0, 1.0]; width * height],
            textures: HashMap::new(),
        }
    }

    fn textures(&mut self, delta: &egui::TexturesDelta) {
        for (id, delta) in delta.set.iter().flat_map(|(id, deltas)| deltas.iter().map(move |delta| (id, delta))) {
            let egui::ImageData::Color(image) = &delta.image;
            match delta.pos {
                None => {
                    self.textures.insert(*id, egui::ColorImage::clone(image));
                }
                Some([x, y]) => {
                    let Some(texture) = self.textures.get_mut(id) else { continue };
                    for row in 0..image.size[1] {
                        for column in 0..image.size[0] {
                            let at = (y + row) * texture.size[0] + x + column;
                            texture.pixels[at] = image.pixels[row * image.size[0] + column];
                        }
                    }
                }
            }
        }
    }

    fn paint(&mut self, context: &egui::Context, mut output: egui::FullOutput) {
        self.textures(&output.textures_delta);
        output.textures_delta.clear();
        let pixels_per_point = output.pixels_per_point;
        let textures = std::mem::take(&mut self.textures);
        for primitive in context.tessellate(output.shapes, pixels_per_point) {
            let egui::epaint::Primitive::Mesh(mesh) = &primitive.primitive else { continue };
            let clip = primitive.clip_rect;
            let clip = [
                (clip.left() * pixels_per_point).max(0.0) as usize,
                (clip.top() * pixels_per_point).max(0.0) as usize,
                ((clip.right() * pixels_per_point).ceil() as usize).min(self.width),
                ((clip.bottom() * pixels_per_point).ceil() as usize).min(self.height),
            ];
            let texture = textures.get(&mesh.texture_id);
            for triangle in mesh.indices.chunks_exact(3) {
                let [a, b, c] = [0, 1, 2].map(|corner| mesh.vertices[triangle[corner] as usize]);
                self.triangle([a, b, c], pixels_per_point, clip, texture);
            }
        }
        self.textures = textures;
    }

    fn triangle(
        &mut self,
        corners: [egui::epaint::Vertex; 3],
        pixels_per_point: f32,
        clip: [usize; 4],
        texture: Option<&egui::ColorImage>,
    ) {
        let at = corners.map(|corner| corner.pos * pixels_per_point);
        let edge = |a: Pos2, b: Pos2, p: Pos2| (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
        let area = edge(at[0], at[1], at[2]);
        if area.abs() < 1e-9 {
            return;
        }
        let low = at.iter().fold(Pos2::new(f32::MAX, f32::MAX), |low, p| low.min(*p));
        let high = at.iter().fold(Pos2::new(f32::MIN, f32::MIN), |high, p| high.max(*p));
        let x0 = (low.x.floor().max(0.0) as usize).max(clip[0]);
        let y0 = (low.y.floor().max(0.0) as usize).max(clip[1]);
        let x1 = ((high.x.ceil().max(0.0)) as usize).min(clip[2]);
        let y1 = ((high.y.ceil().max(0.0)) as usize).min(clip[3]);
        for y in y0..y1 {
            for x in x0..x1 {
                let p = Pos2::new(x as f32 + 0.5, y as f32 + 0.5);
                let w = [
                    edge(at[1], at[2], p) / area,
                    edge(at[2], at[0], p) / area,
                    edge(at[0], at[1], p) / area,
                ];
                if w.iter().any(|&weight| weight < 0.0) {
                    continue;
                }
                let mut source = [0.0f32; 4];
                let mut uv = Vec2::ZERO;
                for (corner, weight) in corners.iter().zip(w) {
                    for (channel, value) in source.iter_mut().zip(corner.color.to_array()) {
                        *channel += f32::from(value) / 255.0 * weight;
                    }
                    uv += corner.uv.to_vec2() * weight;
                }
                if let Some(texture) = texture {
                    let tx = ((uv.x * texture.size[0] as f32) as usize).min(texture.size[0] - 1);
                    let ty = ((uv.y * texture.size[1] as f32) as usize).min(texture.size[1] - 1);
                    let texel = texture.pixels[ty * texture.size[0] + tx].to_array();
                    for (channel, value) in source.iter_mut().zip(texel) {
                        *channel *= f32::from(value) / 255.0;
                    }
                }
                let pixel = &mut self.pixels[y * self.width + x];
                let keep = 1.0 - source[3];
                for (channel, value) in pixel.iter_mut().zip(source) {
                    *channel = value + *channel * keep;
                }
            }
        }
    }

    fn save(&self, path: &Path) {
        let rgb: Vec<u8> = self
            .pixels
            .iter()
            .flat_map(|pixel| [0, 1, 2].map(|channel| (pixel[channel].clamp(0.0, 1.0) * 255.0).round() as u8))
            .collect();
        let file = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
        let mut encoder = png::Encoder::new(file, self.width as u32, self.height as u32);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header().unwrap().write_image_data(&rgb).unwrap();
    }
}

const PIXELS_PER_POINT: f32 = 2.0;

/// A context drawing at the pictures' scale with the editor's fonts.
fn picture_context() -> egui::Context {
    let context = egui::Context::default();
    context.set_pixels_per_point(PIXELS_PER_POINT);
    context.set_fonts(editor_fonts());
    context
}

/// Paints `contents` on `size` points three times, so fonts and layout settle, and keeps the
/// last.
fn picture(size: Vec2, path: &Path, mut contents: impl FnMut(&mut egui::Ui)) {
    let context = picture_context();
    let mut canvas = Canvas::new(size, PIXELS_PER_POINT);
    for pass in 0..3 {
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                ..Default::default()
            },
            &mut contents,
        );
        if pass < 2 {
            canvas.textures(&output.textures_delta);
            output.textures_delta.clear();
            continue;
        }
        canvas.paint(&context, output);
    }
    canvas.save(path);
}

/// One line per row: the color of every 24th column, as `#rrggbb`, `-` for none.
fn color_summary(capture: &CaptureUi, settings: &SessionSettings) -> String {
    let Some(palette) = spectrum::waveform_palette(settings) else {
        return "single color\n".into();
    };
    let (rows, bins) = capture.color_rows();
    rows.iter()
        .map(|&row| {
            let rate = capture.timeline.sample_rate;
            let colors =
                capture.waveform_colors.row_colors(row, &capture.peak_frames, bins, &palette, rate);
            let line: Vec<_> = colors
                .iter()
                .step_by(24)
                .map(|color| color.map_or("-".to_owned(), |color| {
                    let [r, g, b, _] = color.to_array();
                    format!("#{r:02x}{g:02x}{b:02x}")
                }))
                .collect();
            format!("{row:?} ({bins} columns): {}\n", line.join(" "))
        })
        .collect()
}

#[test]
#[ignore = "writes pictures into BACKTRACKING_PLUGIN_SHOTS"]
fn shots() {
    let folder = std::path::PathBuf::from(
        std::env::var_os("BACKTRACKING_PLUGIN_SHOTS").expect("set BACKTRACKING_PLUGIN_SHOTS"),
    );
    std::fs::create_dir_all(&folder).unwrap();
    let mono = |signal: fn(usize) -> f32, seconds| captured(1, Vec::new(), seconds, move |_, frame| signal(frame));
    let single = SessionSettings::default();
    let stereo = SessionSettings { waveform_layout: WaveformLayout::Stereo, ..frequency() };
    let mut log = String::new();
    let mut shoot = |name: &str, session: &Arc<Session>, settings: &SessionSettings, selection: Option<(f64, f64)>| {
        let mut capture = editor(session, settings);
        if let Some((from, to)) = selection {
            let frames = capture.peak_frames.clone();
            let at = |fraction: f64| frames.start + ((frames.end - frames.start) as f64 * fraction) as u64;
            capture.selection = Some(at(from)..at(to));
        }
        log += &format!("== {name}\n{}", color_summary(&capture, settings));
        picture(SCREEN, &folder.join(format!("{name}.png")), |ui| {
            capture.draw_waveform(ui, PLOT, capture.seconds, false, settings);
        });
    };
    let (low, _low_worker) = mono(|frame| sine(70.0, frame), 4.0);
    shoot("low-tone-70hz", &low, &frequency(), None);
    let (high, _high_worker) = mono(|frame| sine(9_000.0, frame), 4.0);
    shoot("high-tone-9khz", &high, &frequency(), None);
    let (swept, _swept_worker) = mono(|frame| sweep(6.0, frame), 6.0);
    shoot("sweep-40hz-18khz", &swept, &frequency(), None);
    shoot("sweep-40hz-18khz-single-color", &swept, &single, None);
    shoot("sweep-40hz-18khz-selection", &swept, &frequency(), Some((0.35, 0.65)));
    shoot("sweep-40hz-18khz-selection-single-color", &swept, &single, Some((0.35, 0.65)));
    let (noisy, _noisy_worker) = mono(noise, 4.0);
    shoot("white-noise", &noisy, &frequency(), None);
    let (pair, _pair_worker) = captured(2, Vec::new(), 4.0, |channel, frame| {
        if channel == 0 { sine(70.0, frame) } else { sine(9_000.0, frame) }
    });
    shoot("stereo-left-70hz-right-9khz", &pair, &stereo, None);
    // 5.1: each channel its own tone, louder to softer, so the L and R rows mix them.
    let labels = crate::surround::speaker_labels(&[0, 1, 2, 3, 4, 5]);
    let tones = [60.0, 9_000.0, 700.0, 40.0, 2_500.0, 14_000.0];
    let (wide, _wide_worker) = captured(6, labels, 4.0, |channel, frame| sine(tones[channel], frame));
    shoot("surround-5-1", &wide, &frequency(), None);
    shoot("surround-5-1-selection", &wide, &frequency(), Some((0.2, 0.5)));
    // A gradient of the user's with a stop too dark to see, on the Appearance page.
    let mut appearance = SessionSettings {
        waveform_gradient: WaveformGradient::new([
            GradientStop { position: 0.0, rgb: [250, 200, 40] },
            GradientStop { position: 0.45, rgb: [30, 10, 60] },
            GradientStop { position: 1.0, rgb: [40, 200, 90] },
        ]),
        ..frequency()
    };
    let mut tab = OptionsTab::Appearance;
    picture(OPTIONS_EDITOR_SIZE, &folder.join("options-appearance.png"), |ui| {
        let accent = accent_color(appearance.accent);
        options_layout(ui, &mut tab, accent, |ui, _| appearance_page(ui, &mut appearance, false));
    });
    let gradient = appearance.waveform_gradient.clone();
    shoot("sweep-user-gradient", &swept, &SessionSettings { waveform_gradient: gradient, ..frequency() }, None);
    std::fs::write(folder.join("colors.txt"), &log).unwrap();
    println!("{log}");
}
