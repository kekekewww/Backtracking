//! Direction grid and delay-aligned interpolation shared with the offline evaluator.

pub(super) const SMOOTHSTEPS: usize = 0;
pub(super) const AZIMUTHS: usize = 37;
pub(super) const ELEVATIONS: [f32; 24] = [
    -90.0, -81.0, -75.0, -64.8, -60.0, -54.0, -45.0, -35.3, -30.0, -25.0, -17.5, -15.0, 0.0, 15.0,
    17.5, 25.0, 30.0, 35.3, 45.0, 54.0, 60.0, 64.8, 75.0, 90.0,
];
pub(super) const DIRECTIONS: usize = (ELEVATIONS.len() - 2) * AZIMUTHS + 2;

pub(super) fn row(elevation: usize, azimuth: usize) -> usize {
    match elevation {
        0 => 0,
        e if e == ELEVATIONS.len() - 1 => DIRECTIONS - 1,
        e => 1 + (e - 1) * AZIMUTHS + azimuth,
    }
}

pub(super) struct Cell {
    rows: [usize; 4],
    weights: [f32; 4],
}

pub(super) fn cell(azimuth: f32, elevation: f32, smoothing: usize) -> Cell {
    let az = azimuth.abs() / 5.0;
    let a = (az as usize).min(AZIMUTHS - 2);
    let e = ELEVATIONS
        .partition_point(|&angle| angle <= elevation)
        .saturating_sub(1)
        .min(ELEVATIONS.len() - 2);
    let mut x = az - a as f32;
    let mut y = ((elevation - ELEVATIONS[e]) / (ELEVATIONS[e + 1] - ELEVATIONS[e])).clamp(0.0, 1.0);
    for _ in 0..smoothing {
        x = x * x * (3.0 - 2.0 * x);
        y = y * y * (3.0 - 2.0 * y);
    }
    Cell {
        rows: [row(e, a), row(e, a + 1), row(e + 1, a), row(e + 1, a + 1)],
        weights: [(1.0 - x) * (1.0 - y), x * (1.0 - y), (1.0 - x) * y, x * y],
    }
}

pub(super) fn blend(
    cell: &Cell,
    ear: usize,
    samples: &[f32],
    delays: &[usize],
    output: &mut [f32],
) {
    let n = output.len();
    let ears = cell.rows.map(|r| r * 2 + ear);
    // Copying a node also avoids rounding a measured response.
    if let Some(i) = cell.weights.iter().position(|&w| w == 1.0) {
        output.copy_from_slice(&samples[ears[i] * n..(ears[i] + 1) * n]);
        return;
    }
    // Interpolate the relative delay to keep the fractional part accurate at high rates.
    let base = delays[ears[0]] as isize;
    let offset: f32 = ears
        .iter()
        .zip(cell.weights)
        .map(|(&k, w)| (delays[k] as isize - base) as f32 * w)
        .sum();
    let floor = offset.floor();
    let anchor = base + floor as isize;
    output.fill(0.0);
    for (k, weight) in ears.into_iter().zip(cell.weights) {
        if weight == 0.0 {
            continue;
        }
        let shift = anchor - delays[k] as isize;
        let dst = shift.max(0) as usize;
        let src = (-shift).max(0) as usize;
        let len = n - dst.max(src);
        for (out, &tap) in output[dst..dst + len]
            .iter_mut()
            .zip(&samples[k * n + src..k * n + src + len])
        {
            *out += tap * weight;
        }
    }
    // Cubic Lagrange fractional delay at t-f, using zero extension at both ends.
    let f = offset - floor;
    if f != 0.0 {
        let c0 = -f * (1.0 - f) * (2.0 - f) / 6.0;
        let c1 = (1.0 + f) * (1.0 - f) * (2.0 - f) / 2.0;
        let c2 = (1.0 + f) * f * (2.0 - f) / 2.0;
        let c3 = -(1.0 + f) * f * (1.0 - f) / 6.0;
        let mut next = 0.0;
        for i in (0..n).rev() {
            let current = output[i];
            let prev = if i >= 1 { output[i - 1] } else { 0.0 };
            let prev2 = if i >= 2 { output[i - 2] } else { 0.0 };
            output[i] = c0 * next + c1 * current + c2 * prev + c3 * prev2;
            next = current;
        }
    }
}

pub(super) fn level(taps: &[f32]) -> f32 {
    taps.iter().map(|v| v * v).sum::<f32>().sqrt()
}

// Interpolate the measured FIR energy as well as its shape. This depends only
// on the direction and the bank; it never depends on the audio being rendered.
pub(super) fn match_level(cell: &Cell, ear: usize, levels: &[f32], output: &mut [f32]) {
    if cell.weights.contains(&1.0) {
        return;
    }
    let target = weighted_value(cell, ear, levels);
    let actual = level(output);
    if actual > 0.0 {
        let gain = target / actual;
        for tap in output {
            *tap *= gain;
        }
    }
}

pub(super) fn weighted_value(cell: &Cell, ear: usize, values: &[f32]) -> f32 {
    cell.rows
        .iter()
        .zip(cell.weights)
        .map(|(&r, w)| w * values[2 * r + ear])
        .sum()
}
