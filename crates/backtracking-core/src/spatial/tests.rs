use super::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;

struct CountAllocations;

thread_local! {
    static ALLOCATION_COUNTS: Cell<Option<(usize, usize)>> = const { Cell::new(None) };
}

fn count_allocation(allocated: usize, freed: usize) {
    let _ = ALLOCATION_COUNTS.try_with(|counts| {
        if let Some((a, f)) = counts.get() {
            counts.set(Some((a + allocated, f + freed)));
        }
    });
}

// Only this test thread's allocations are counted; parallel tests can run normally.
unsafe impl GlobalAlloc for CountAllocations {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count_allocation(1, 0);
        // SAFETY: the layout and return value are forwarded to the system allocator.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count_allocation(1, 0);
        // SAFETY: the layout and return value are forwarded to the system allocator.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        count_allocation(0, 1);
        // SAFETY: the pointer and its allocation layout are forwarded unchanged.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count_allocation(1, 1);
        // SAFETY: the pointer, layout and requested size are forwarded unchanged.
        unsafe { System.realloc(ptr, layout, size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountAllocations = CountAllocations;

fn model() -> Arc<HeadModel> {
    Arc::new(HeadModel::new(48_000).unwrap())
}

fn fixed(labels: &[&str]) -> Vec<BinauralSource> {
    speaker_directions(labels)
        .into_iter()
        .map(BinauralSource::Fixed)
        .collect()
}

fn key(frame: u64, x: f32, y: f32, z: f32) -> PositionKey {
    PositionKey {
        frame,
        x,
        y,
        z,
        size: 0.0,
        gain: 1.0,
    }
}

fn signal(frames: usize, channels: usize) -> Vec<f32> {
    let mut random = 0x1234_5678u32;
    (0..frames * channels)
        .map(|_| {
            random ^= random << 13;
            random ^= random >> 17;
            random ^= random << 5;
            (random as f64 / u32::MAX as f64 * 0.4 - 0.2) as f32
        })
        .collect()
}

fn clip(sources: &[BinauralSource], input: &[f32]) -> Vec<f32> {
    render_binaural(model(), sources, None, 0, input).unwrap()
}

fn energy(output: &[f32], ear: usize) -> f64 {
    output
        .chunks_exact(2)
        .map(|frame| f64::from(frame[ear]).powi(2))
        .sum()
}

#[test]
fn front_impulses_have_correct_ear_order_and_exact_mirrors() {
    let mut impulse = vec![0.0; 512];
    impulse[0] = 1.0;
    let left = clip(&fixed(&["FL"]), &impulse);
    let right = clip(&fixed(&["FR"]), &impulse);
    for (left, right) in left.chunks_exact(2).zip(right.chunks_exact(2)) {
        assert_eq!(left[0], right[1]);
        assert_eq!(left[1], right[0]);
    }
    let peak = |ear: usize| {
        left.chunks_exact(2)
            .enumerate()
            .max_by(|(_, a), (_, b)| a[ear].abs().total_cmp(&b[ear].abs()))
            .unwrap()
            .0
    };
    assert!(peak(0) < peak(1), "FL must reach the left ear first");
    assert!(energy(&left, 0) > energy(&left, 1));
    let centre = clip(&fixed(&["FC"]), &impulse);
    assert!(centre.chunks_exact(2).all(|frame| frame[0] == frame[1]));
}

#[test]
fn lfe_and_unknown_labels_are_unfiltered_equal_ears_with_fixed_trim() {
    let input = signal(777, 1);
    for label in ["LFE", "LFE1", "LFE2", "Object 1", "Channel 20"] {
        let output = clip(&fixed(&[label]), &input);
        for (sample, frame) in input.iter().zip(output.chunks_exact(2)) {
            let expected = sample * std::f32::consts::FRAC_1_SQRT_2 * 0.5;
            assert!((frame[0] - expected).abs() < 1e-7, "{label}");
            assert_eq!(frame[0], frame[1]);
        }
    }
}

#[test]
fn separately_rendered_channels_sum_to_the_mix_without_normalisation() {
    let mut sources = fixed(&[
        "FL", "FR", "FC", "LFE", "Ls", "Rs", "Lb", "Rb", "TFL", "TFR", "TBL", "TBR",
    ]);
    sources.push(BinauralSource::Moving(vec![
        key(0, -0.9, 0.6, 0.1),
        key(500, 0.9, 0.6, 0.7),
    ]));
    sources.push(BinauralSource::Fixed(None));
    let input = signal(1_103, sources.len());
    let model = model();
    let mixed = render_binaural(model.clone(), &sources, None, 0, &input).unwrap();
    let mut summed = vec![0.0; mixed.len()];
    for channel in 0..sources.len() {
        let selected =
            render_binaural(model.clone(), &sources, Some(&[channel]), 0, &input).unwrap();
        for (total, value) in summed.iter_mut().zip(selected) {
            *total += value;
        }
    }
    let error = summed
        .iter()
        .zip(mixed)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(error <= 1e-6, "maximum mix error {error}");
    let silent = render_binaural(model, &sources, Some(&[]), 0, &input).unwrap();
    assert!(silent.iter().all(|value| *value == 0.0));
}

#[test]
fn block_sizes_do_not_change_fixed_or_moving_output() {
    let sources = [
        BinauralSource::Fixed(Some(Direction {
            azimuth: -30.0,
            elevation: 0.0,
        })),
        BinauralSource::Moving(vec![
            key(90, -0.8, 0.7, 0.2),
            key(403, 0.8, 0.2, -0.1),
            key(900, -0.2, -0.9, 0.6),
        ]),
        BinauralSource::Fixed(None),
    ];
    let input = signal(1_039, sources.len());
    let model = model();
    let whole = render_binaural(model.clone(), &sources, None, 103, &input).unwrap();
    for blocks in [
        &[1][..],
        &[7],
        &[127],
        &[128],
        &[129],
        &[256],
        &[911],
        &[3, 260, 1, 95],
    ] {
        let mut renderer = BinauralRenderer::new(model.clone(), &sources, None, 103).unwrap();
        let mut output = vec![0.0; whole.len()];
        let mut first = 0;
        let mut block = 0;
        while first < input.len() / sources.len() {
            let end = (first + blocks[block % blocks.len()]).min(input.len() / sources.len());
            renderer
                .render(
                    &input[first * sources.len()..end * sources.len()],
                    &mut output[first * 2..end * 2],
                )
                .unwrap();
            first = end;
            block += 1;
        }
        assert_eq!(output, whole, "caller block sizes {blocks:?}");
        assert_eq!(renderer.position(), 103 + 1_039);
    }
}

#[test]
fn a_stationary_position_track_equals_the_fixed_source() {
    let input = signal(1_083, 1);
    // atan2(-1, 1) is exactly the -45 degree grid direction.
    let stationary = [BinauralSource::Moving(vec![
        key(0, -1.0, 1.0, 0.0),
        key(173, -1.0, 1.0, 0.0),
        key(700, -1.0, 1.0, 0.0),
    ])];
    let fixed = [BinauralSource::Fixed(Some(Direction {
        azimuth: -45.0,
        elevation: 0.0,
    }))];
    let a = clip(&stationary, &input);
    let b = clip(&fixed, &input);
    let error = a
        .iter()
        .zip(b)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(error < 1e-7, "stationary versus fixed error {error}");
}

#[test]
fn position_interpolation_is_cartesian_at_a_clip_start() {
    // Halfway between these points is (0, 0.5, 0.5), hence azimuth 0/elevation 45.
    // Averaging endpoint azimuth/elevation instead would produce another direction.
    let source = [BinauralSource::Moving(vec![
        key(0, -1.0, 1.0, 0.0),
        key(100, 1.0, 0.0, 1.0),
    ])];
    let fixed = [BinauralSource::Fixed(Some(Direction {
        azimuth: 0.0,
        elevation: 45.0,
    }))];
    let input = signal(128, 1);
    let model = model();
    let moving = render_binaural(model.clone(), &source, None, 50, &input).unwrap();
    let stationary = render_binaural(model, &fixed, None, 50, &input).unwrap();
    assert_eq!(moving, stationary);
}

#[test]
fn fixed_render_matches_an_independent_scalar_convolution() {
    let model = model();
    let direction = Direction {
        azimuth: -37.0,
        elevation: 22.5,
    };
    let mut left = vec![0.0; model.taps()];
    let mut right = left.clone();
    model.filter(Some(direction), &mut left, &mut right);
    let input = signal(777, 1);
    let actual = render_binaural(
        model,
        &[BinauralSource::Fixed(Some(direction))],
        None,
        0,
        &input,
    )
    .unwrap();
    for (frame, actual) in actual.chunks_exact(2).enumerate() {
        for (ear, filter) in [&left, &right].into_iter().enumerate() {
            let expected = (0..filter.len().min(frame + 1))
                .map(|tap| f64::from(input[frame - tap]) * f64::from(filter[tap]))
                .sum::<f64>()
                * 0.5;
            assert!((f64::from(actual[ear]) - expected).abs() < 1e-6);
        }
    }
}

#[test]
fn movement_crosses_ears_with_bounded_sample_steps() {
    let frames = 16_384;
    let input: Vec<_> = (0..frames)
        .map(|frame| {
            let t = frame as f32 / 48_000.0;
            0.10 * (std::f32::consts::TAU * 731.0 * t).sin()
                + 0.10 * (std::f32::consts::TAU * 1_753.0 * t).sin()
        })
        .collect();
    let output = clip(
        &[BinauralSource::Moving(vec![
            key(0, -1.0, 0.4, 0.0),
            key(2_048, -1.0, 0.4, 0.0),
            key(14_336, 1.0, 0.4, 0.0),
        ])],
        &input,
    );
    let first = &output[512 * 2..2_048 * 2];
    let last = &output[14_848 * 2..];
    let first_ild = 10.0 * (energy(first, 0) / energy(first, 1)).log10();
    let last_ild = 10.0 * (energy(last, 0) / energy(last, 1)).log10();
    assert!(first_ild > 1.0, "left start ILD {first_ild} dB");
    assert!(last_ild < -1.0, "right end ILD {last_ild} dB");
    let max_step = output
        .chunks_exact(2)
        .zip(output[2..].chunks_exact(2))
        .flat_map(|(a, b)| [(a[0] - b[0]).abs(), (a[1] - b[1]).abs()])
        .fold(0.0f32, f32::max);
    // This bound includes the natural slope of the 0.2-peak two-tone test signal.
    assert!(
        max_step < 0.10,
        "largest consecutive-sample step {max_step}"
    );
    println!(
        "moving source: start ILD {first_ild:.3} dB, end ILD {last_ild:.3} dB, maximum sample step {max_step:.8} FS (bound 0.10)"
    );
}

#[test]
fn missing_positions_and_listener_origin_use_the_equal_ear_fallback() {
    let input = signal(500, 1);
    let expected = clip(&[BinauralSource::Fixed(None)], &input);
    for keys in [vec![], vec![key(0, 0.0, 0.0, 0.0)]] {
        assert_eq!(clip(&[BinauralSource::Moving(keys)], &input), expected);
    }
    let future = clip(
        &[BinauralSource::Moving(vec![key(384, 1.0, 0.0, 0.0)])],
        &input,
    );
    assert_eq!(&future[..384 * 2], &expected[..384 * 2]);
}

#[test]
fn position_gain_is_applied_and_distance_and_size_are_ignored() {
    let input = signal(513, 1);
    let original = clip(
        &[BinauralSource::Moving(vec![key(0, -0.5, 0.5, 0.0)])],
        &input,
    );
    let mut changed = key(0, -1.0, 1.0, 0.0);
    changed.size = 1.0;
    changed.gain = 0.25;
    let actual = clip(&[BinauralSource::Moving(vec![changed])], &input);
    for (a, b) in actual.iter().zip(original) {
        assert!((a - b * 0.25).abs() <= 1e-7);
    }
}

#[test]
fn length_is_preserved_and_filter_tails_are_cut() {
    let sources = fixed(&["FL", "FR"]);
    for frames in [0, 1, 2, 127, 128, 129, 257] {
        let mut input = vec![0.0; frames * 2];
        if let Some(last) = input.last_mut() {
            *last = 1.0;
        }
        let output = clip(&sources, &input);
        assert_eq!(output.len(), frames * 2);
        assert!(output.iter().all(|sample| sample.is_finite()));
    }
}

#[test]
fn reset_restores_a_fresh_engine_with_empty_filter_history() {
    let sources = [BinauralSource::Moving(vec![
        key(0, -1.0, 0.7, 0.0),
        key(999, 1.0, 0.7, 0.2),
    ])];
    let input = signal(777, 1);
    let model = model();
    let mut renderer = BinauralRenderer::new(model.clone(), &sources, None, 0).unwrap();
    let mut output = vec![0.0; input.len() * 2];
    renderer.render(&input, &mut output).unwrap();
    renderer.reset(195);
    assert_eq!(renderer.position(), 195);
    renderer.render(&input, &mut output).unwrap();
    let expected = render_binaural(model, &sources, None, 195, &input).unwrap();
    assert_eq!(output, expected);
}

#[test]
fn rendering_and_reset_do_not_allocate_or_deallocate() {
    let sources = [
        BinauralSource::Moving(vec![key(0, -1.0, 0.4, 0.0), key(1_000, 1.0, 0.4, 0.8)]),
        BinauralSource::Fixed(None),
    ];
    let mut renderer = BinauralRenderer::new(model(), &sources, None, 0).unwrap();
    let input = signal(257, sources.len());
    let mut output = vec![0.0; 257 * 2];
    ALLOCATION_COUNTS.with(|count| count.set(Some((0, 0))));
    for _ in 0..8 {
        renderer.render(&input, &mut output).unwrap();
    }
    renderer.reset(13);
    renderer.render(&input, &mut output).unwrap();
    let counts = ALLOCATION_COUNTS.with(|count| count.replace(None).unwrap());
    assert_eq!(counts, (0, 0), "(allocation calls, deallocation calls)");
}

#[test]
fn supported_rates_preserve_length_symmetry_and_block_independence() {
    for rate in [8_000, 44_100, 96_000, 384_000] {
        let model = Arc::new(HeadModel::new(rate).unwrap());
        assert_eq!(model.sample_rate(), rate);
        assert!(model.taps() > 0);
        let sources = fixed(&["FC"]);
        let input = signal(193, 1);
        let expected = render_binaural(model.clone(), &sources, None, 0, &input).unwrap();
        assert_eq!(expected.len(), 386);
        assert!(expected.chunks_exact(2).all(|frame| frame[0] == frame[1]));
        let mut renderer = BinauralRenderer::new(model, &sources, None, 0).unwrap();
        let mut output = vec![0.0; expected.len()];
        for (input, output) in input.chunks(17).zip(output.chunks_mut(34)) {
            renderer.render(input, output).unwrap();
        }
        assert_eq!(output, expected, "sample rate {rate}");
    }
    for rate in [0, 7_999, 384_001, u32::MAX] {
        assert!(matches!(
            HeadModel::new(rate),
            Err(RenderError::InvalidSampleRate)
        ));
    }
}

#[test]
fn invalid_buffers_and_source_definitions_are_rejected() {
    let model = model();
    let sources = fixed(&["FL", "FR"]);
    let mut renderer = BinauralRenderer::new(model.clone(), &sources, None, 0).unwrap();
    let mut unchanged = [7.0; 2];
    assert_eq!(
        renderer.render(&[0.0; 3], &mut unchanged),
        Err(RenderError::InvalidBuffer)
    );
    assert_eq!(unchanged, [7.0; 2]);
    assert_eq!(
        renderer.render(&[0.0; 4], &mut [0.0; 3]),
        Err(RenderError::InvalidBuffer)
    );
    assert_eq!(renderer.position(), 0);
    assert!(BinauralRenderer::new(model.clone(), &[], None, 0).is_err());
    assert!(matches!(
        BinauralRenderer::new(model.clone(), &sources, Some(&[2]), 0),
        Err(RenderError::InvalidSelection)
    ));
    assert!(matches!(
        BinauralRenderer::new(model.clone(), &sources, Some(&[0, 0]), 0),
        Err(RenderError::InvalidSelection)
    ));
    assert!(BinauralRenderer::new(
        model.clone(),
        &[BinauralSource::Fixed(Some(Direction {
            azimuth: f32::NAN,
            elevation: 0.0
        }))],
        None,
        0,
    )
    .is_err());
    for keys in [
        vec![key(10, 0.0, 1.0, 0.0), key(9, 0.0, 1.0, 0.0)],
        vec![key(10, 0.0, 1.0, 0.0), key(10, 0.0, 1.0, 0.0)],
        vec![key(0, f32::INFINITY, 0.0, 0.0)],
    ] {
        assert!(matches!(
            BinauralRenderer::new(model.clone(), &[BinauralSource::Moving(keys)], None, 0),
            Err(RenderError::InvalidPositionKeys)
        ));
    }
    for (size, gain) in [
        (-0.1, 1.0),
        (1.1, 1.0),
        (0.0, -0.1),
        (f32::NAN, 1.0),
        (0.0, f32::INFINITY),
    ] {
        let mut invalid = key(0, 1.0, 0.0, 0.0);
        invalid.size = size;
        invalid.gain = gain;
        assert!(matches!(
            BinauralRenderer::new(
                model.clone(),
                &[BinauralSource::Moving(vec![invalid])],
                None,
                0
            ),
            Err(RenderError::InvalidPositionKeys)
        ));
    }
    let mut renderer = BinauralRenderer::new(model, &sources, None, u64::MAX).unwrap();
    let mut output = [7.0; 2];
    assert_eq!(
        renderer.render(&[0.0; 2], &mut output),
        Err(RenderError::TimelineOverflow)
    );
    assert_eq!(renderer.position(), u64::MAX);
    assert_eq!(output, [7.0; 2]);
}

#[test]
fn orbit_has_bounded_changes_in_interaural_level() {
    // Identical isolated impulses sample the moving renderer every 0.96 degrees.
    const PERIOD: usize = 512;
    const STEPS: usize = 375;
    for (elevation, bound) in [(0.0_f32, 0.82), (30.0, 0.95)] {
        let (height, horizontal) = elevation.to_radians().sin_cos();
        let keys = (0..=STEPS)
            .map(|i| {
                let angle = std::f32::consts::TAU * i as f32 / STEPS as f32;
                key(
                    (i * PERIOD) as u64,
                    horizontal * angle.sin(),
                    horizontal * angle.cos(),
                    height,
                )
            })
            .collect();
        let mut impulses = vec![0.0; (STEPS + 1) * PERIOD];
        for period in impulses.chunks_exact_mut(PERIOD) {
            period[0] = 1.0;
        }
        let output = clip(&[BinauralSource::Moving(keys)], &impulses);
        let levels: Vec<_> = output
            .chunks_exact(2 * PERIOD)
            .map(|period| 10.0 * (energy(period, 0) / energy(period, 1)).log10())
            .collect();
        assert!(levels.iter().all(|db| db.is_finite()));
        assert!(levels[STEPS / 4] < -3.0 && levels[3 * STEPS / 4] > 3.0);
        let steps: Vec<_> = levels.windows(2).map(|w| (w[0] - w[1]).abs()).collect();
        let rms = (steps.iter().map(|v| v * v).sum::<f64>() / STEPS as f64).sqrt();
        let (index, largest) = steps
            .iter()
            .copied()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap();
        eprintln!("orbit elevation {elevation}, ILD RMS step {rms:.6}, maximum {largest:.6} dB at interval {index}");
        assert!(largest < bound, "maximum ILD step {largest} dB");
    }
}
