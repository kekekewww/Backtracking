//! Offline reduction of the pinned SADIE II D1 OpenJOC asset. No network or dependencies.

#[path = "../../../src/spatial/interpolation.rs"]
mod interpolation;

use std::{error::Error, f64::consts::PI, fs, io, path::Path};

const INPUT_HASH: &str = "78d048a68f84d34051578c262e401e35baa0e718901f85349afe0232f985d4df";
const MEASURED: usize = 8_802;
const TAPS: usize = 256;
const ELEVATIONS: usize = 13;
const ELEVATION_STEP: f64 = 15.0;

#[derive(Clone)]
struct Row {
    direction: [f64; 3],
    taps: [f32; TAPS * 2],
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

struct Cursor<'a>(&'a [u8]);

impl<'a> Cursor<'a> {
    fn take(&mut self, count: usize) -> io::Result<&'a [u8]> {
        let (head, tail) = self
            .0
            .split_at_checked(count)
            .ok_or_else(|| invalid("truncated asset"))?;
        self.0 = tail;
        Ok(head)
    }

    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> io::Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
}

fn parse(bytes: &[u8]) -> io::Result<Vec<Row>> {
    let mut cursor = Cursor(bytes);
    if cursor.take(8)? != b"OJHRTF2\0" || cursor.u32()? != 2 || cursor.u32()? != 1 {
        return Err(invalid("expected version 2 D1 asset"));
    }
    let payload_length = cursor.u64()?;
    let checksum = cursor.take(32)?;
    if payload_length != cursor.0.len() as u64 || checksum != sha256(cursor.0) {
        return Err(invalid("payload length or SHA-256 mismatch"));
    }
    if cursor.take(8)? != b"OJRIR2\0\0" || cursor.u32()? != 48_000 {
        return Err(invalid("expected 48 kHz record table"));
    }
    let count = cursor.u32()? as usize;
    if count == 0 || count > MEASURED + 15 || cursor.u32()? != TAPS as u32 {
        return Err(invalid("unexpected direction or tap count"));
    }
    for _ in 0..5 {
        if cursor.u32()? != 1 {
            return Err(invalid("unsupported record encoding"));
        }
    }
    let mut rows = Vec::with_capacity(count);
    for _ in 0..count {
        let mut direction = [0.0; 3];
        for value in &mut direction {
            *value = f64::from_bits(cursor.u64()?);
        }
        let norm = direction.iter().map(|v| v * v).sum::<f64>();
        if !norm.is_finite() || (norm - 1.0).abs() > 1e-8 || cursor.u32()? != TAPS as u32 {
            return Err(invalid("invalid direction or row length"));
        }
        // The pinned asset contains no separate delays. Refuse rather than lose them.
        if cursor.u32()? != 0 || cursor.u32()? != 0 {
            return Err(invalid("nonzero separate sample delay"));
        }
        let mut taps = [0.0; TAPS * 2];
        for value in &mut taps {
            *value = f32::from_bits(cursor.u32()?);
            if !value.is_finite() {
                return Err(invalid("nonfinite tap"));
            }
        }
        rows.push(Row { direction, taps });
    }
    if !cursor.0.is_empty() {
        return Err(invalid("trailing data"));
    }
    Ok(rows)
}

fn direction(azimuth: f64, elevation: f64) -> [f64; 3] {
    let a = azimuth * PI / 180.0;
    let e = elevation * PI / 180.0;
    [a.sin() * e.cos(), a.cos() * e.cos(), e.sin()]
}

fn nearest(rows: &[Row], target: [f64; 3]) -> (&Row, f64) {
    rows.iter()
        .map(|row| {
            let dot = row
                .direction
                .iter()
                .zip(target)
                .map(|(a, b)| a * b)
                .sum::<f64>();
            (row, dot)
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap()
}

struct Grid {
    azimuth_step: usize,
    rows: Vec<[f32; TAPS * 2]>,
}

impl Grid {
    fn new(rows: &[Row], azimuth_step: usize) -> Self {
        let mut reduced = Vec::new();
        let mut largest_angle: f64 = 0.0;
        let mut rounding_max: f64 = 0.0;
        for e in 0..ELEVATIONS {
            for a in 0..=180 / azimuth_step {
                let azimuth = (a * azimuth_step) as f64;
                let elevation = e as f64 * ELEVATION_STEP - 90.0;
                // All copies of a pole must use the same measurement.
                let pole = e == 0 || e == ELEVATIONS - 1;
                let target = direction(if pole { 0.0 } else { azimuth }, elevation);
                let (source, dot) = nearest(rows, target);
                largest_angle = largest_angle.max(dot.clamp(-1.0, 1.0).acos().to_degrees());
                let mut taps = source.taps;
                if pole || a == 0 || a == 180 / azimuth_step {
                    for t in 0..TAPS {
                        let average = (taps[t] as f64 + taps[t + TAPS] as f64) * 0.5;
                        taps[t] = average as f32;
                        taps[t + TAPS] = average as f32;
                        rounding_max = rounding_max.max((average - taps[t] as f64).abs());
                    }
                }
                reduced.push(taps);
            }
        }
        println!("grid {azimuth_step}x15: {} rows, {} bytes, maximum grid-to-measurement angle {largest_angle:.9} deg, f32 symmetry rounding max {rounding_max:.12}", reduced.len(), reduced.len() * TAPS * 2 * 4);
        Self {
            azimuth_step,
            rows: reduced,
        }
    }

    fn at(&self, azimuth: f64, elevation: f64) -> [f32; TAPS * 2] {
        let columns = 180 / self.azimuth_step + 1;
        let a = azimuth.abs() as f32 / self.azimuth_step as f32;
        let e = (elevation as f32 + 90.0) / ELEVATION_STEP as f32;
        let a0 = (a.floor() as usize).min(columns - 2);
        let e0 = (e.floor() as usize).min(ELEVATIONS - 2);
        let aw = a - a0 as f32;
        let ew = e - e0 as f32;
        let weights = [
            (1.0 - aw) * (1.0 - ew),
            aw * (1.0 - ew),
            (1.0 - aw) * ew,
            aw * ew,
        ];
        let mut result = [0.0; TAPS * 2];
        for (i, value) in result.iter_mut().enumerate() {
            let source = if azimuth < 0.0 {
                (i + TAPS) % (2 * TAPS)
            } else {
                i
            };
            *value = self.rows[e0 * columns + a0][source] * weights[0]
                + self.rows[e0 * columns + a0 + 1][source] * weights[1]
                + self.rows[(e0 + 1) * columns + a0][source] * weights[2]
                + self.rows[(e0 + 1) * columns + a0 + 1][source] * weights[3];
        }
        result
    }
}

fn peak(taps: &[f32]) -> i32 {
    taps.iter()
        .enumerate()
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
        .unwrap()
        .0 as i32
}

fn ild(taps: &[f32]) -> f64 {
    let energy = |ear: &[f32]| ear.iter().map(|&x| (x as f64).powi(2)).sum::<f64>();
    10.0 * (energy(&taps[..TAPS]) / energy(&taps[TAPS..])).log10()
}

const FREQUENCIES: [f64; 8] = [
    200.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 12000.0, 16000.0,
];

fn basis() -> Vec<Vec<(f64, f64)>> {
    FREQUENCIES
        .iter()
        .map(|frequency| {
            (0..TAPS)
                .map(|tap| {
                    let angle = 2.0 * PI * frequency * tap as f64 / 48_000.0;
                    (angle.cos(), angle.sin())
                })
                .collect()
        })
        .collect()
}

fn db(taps: &[f32], wave: &[(f64, f64)]) -> f64 {
    let (re, im) = taps
        .iter()
        .zip(wave)
        .fold((0.0, 0.0), |(re, im), (&tap, &(c, s))| {
            (re + tap as f64 * c, im + tap as f64 * s)
        });
    20.0 * re.hypot(im).max(0.001).log10()
}

fn angles(a: usize, e: usize, held_out: bool) -> (f64, f64) {
    let (az, el) = if held_out {
        (2.5, -82.5)
    } else {
        (1.35, -85.65)
    };
    (az + 5.0 * a as f64, el + 15.0 * e as f64)
}

fn evaluate(
    name: &str,
    rows: &[Row],
    held_out: bool,
    response: impl Fn(f64, f64) -> [f32; 2 * TAPS],
) {
    let mut squared = 0.0;
    let mut reference_energy = 0.0;
    let mut max: f64 = 0.0;
    let mut itd_max = 0;
    let mut itd_squared = 0.0;
    let mut ild_max: f64 = 0.0;
    let mut ild_squared = 0.0;
    let basis = basis();
    let mut spectral_max: f64 = 0.0;
    let mut spectral_squared = 0.0;
    let mut count = 0;
    // Both sets sample the spaces between stored azimuths.
    for e in 0..12 {
        for a in 0..36 {
            for sign in [-1.0, 1.0] {
                let (azimuth, elevation) = angles(a, e, held_out);
                let azimuth = sign * azimuth;
                let (reference, _) = nearest(rows, direction(azimuth, elevation));
                let actual = response(azimuth, elevation);
                for (&value, &expected) in actual.iter().zip(&reference.taps) {
                    let delta = value as f64 - expected as f64;
                    squared += delta * delta;
                    reference_energy += (expected as f64).powi(2);
                    max = max.max(delta.abs());
                }
                let itd = |taps: &[f32]| peak(&taps[..TAPS]) - peak(&taps[TAPS..]);
                let itd_error = (itd(&actual) - itd(&reference.taps)).abs();
                itd_max = itd_max.max(itd_error);
                itd_squared += (itd_error as f64).powi(2);
                let ild_error = (ild(&actual) - ild(&reference.taps)).abs();
                ild_max = ild_max.max(ild_error);
                ild_squared += ild_error.powi(2);
                for ear in 0..2 {
                    for wave in &basis {
                        let range = ear * TAPS..(ear + 1) * TAPS;
                        let difference =
                            db(&actual[range.clone()], wave) - db(&reference.taps[range], wave);
                        spectral_max = spectral_max.max(difference.abs());
                        spectral_squared += difference * difference;
                    }
                }
                count += 1;
            }
        }
    }
    println!("{name}: {count} directions held_out={held_out}; tap max={max:.9}, RMS={:.9}, normalized RMS={:.9}; peak ITD max={itd_max} samples RMS={:.6}; energy ILD max={ild_max:.6} dB RMS={:.6} dB", (squared / (count * TAPS * 2) as f64).sqrt(), (squared / reference_energy).sqrt(), (itd_squared / count as f64).sqrt(), (ild_squared / count as f64).sqrt());
    println!("{name}: magnitude error at {FREQUENCIES:?} Hz, -60dB absolute floor: max={spectral_max:.6} dB RMS={:.6} dB", (spectral_squared / (count * 2 * FREQUENCIES.len()) as f64).sqrt());
}

// Each ear carries its own f32 quantization step followed by 256 little-endian i16 taps.
fn encode(rows: &[Row]) -> (Vec<u8>, Vec<f32>) {
    let mut bytes = Vec::new();
    let mut decoded = Vec::new();
    let mut max_error = 0.0f32;
    let mut squared = 0.0;
    for (e, &el) in interpolation::ELEVATIONS.iter().enumerate() {
        let pole = e == 0 || e + 1 == interpolation::ELEVATIONS.len();
        let columns = if pole { 1 } else { interpolation::AZIMUTHS };
        for a in 0..columns {
            let (source, _) = nearest(rows, direction((a * 5) as f64, el as f64));
            let mut taps = source.taps;
            if pole || a == 0 || a == interpolation::AZIMUTHS - 1 {
                for t in 0..TAPS {
                    let mid = 0.5 * (taps[t] + taps[t + TAPS]);
                    taps[t] = mid;
                    taps[t + TAPS] = mid;
                }
            }
            for ear in taps.as_chunks::<TAPS>().0 {
                let step = ear.iter().map(|v| v.abs()).fold(0.0, f32::max) / 32767.0;
                bytes.extend(step.to_le_bytes());
                for &v in ear {
                    let q = (v / step).round() as i16;
                    bytes.extend(q.to_le_bytes());
                    let y = q as f32 * step;
                    decoded.push(y);
                    max_error = max_error.max((y - v).abs());
                    squared += f64::from(y - v).powi(2);
                }
            }
        }
    }
    println!(
        "quantization max={max_error:.12} RMS={:.12}",
        (squared / decoded.len() as f64).sqrt()
    );
    (bytes, decoded)
}

fn interpolated(samples: &[f32], delays: &[usize], az: f64, el: f64) -> [f32; TAPS * 2] {
    let cell = interpolation::cell(az as f32, el as f32, 0);
    let mut taps = [0.0; TAPS * 2];
    for (ear, out) in taps.as_chunks_mut::<TAPS>().0.iter_mut().enumerate() {
        interpolation::blend(&cell, ear ^ usize::from(az < 0.0), samples, delays, out);
    }
    taps
}

// Fit one delay per ear/node to the measured lags between neighbouring responses.
// Consistent node delays make adjoining interpolation cells agree along shared edges.
fn fit_delays(samples: &[f32]) -> Vec<usize> {
    let count = samples.len() / TAPS;
    let canonical = |k: usize| {
        let r = k / 2;
        if r == 0 || r + 1 == interpolation::DIRECTIONS || {
            let a = (r - 1) % interpolation::AZIMUTHS;
            a == 0 || a + 1 == interpolation::AZIMUTHS
        } {
            k & !1
        } else {
            k
        }
    };
    let mut edges = vec![Vec::new(); count];
    let mut connect = |i: usize, j: usize| {
        let (i, j) = (canonical(i), canonical(j));
        let a = &samples[i * TAPS..(i + 1) * TAPS];
        let b = &samples[j * TAPS..(j + 1) * TAPS];
        let energy = |ear: &[f32]| ear.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>();
        let norm = (energy(a) * energy(b)).sqrt();
        let (lag, corr) = (-16isize..=16)
            .map(|lag| {
                let x = lag.max(0) as usize;
                let y = (-lag).max(0) as usize;
                let dot: f64 = a[x..]
                    .iter()
                    .zip(&b[y..])
                    .map(|(&v, &w)| f64::from(v) * f64::from(w))
                    .sum();
                (lag, dot / norm)
            })
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap();
        let w = corr.max(0.1).powi(4);
        edges[i].push((j, lag as f64, w));
        edges[j].push((i, -(lag as f64), w));
    };
    for e in 0..interpolation::ELEVATIONS.len() {
        for a in 0..interpolation::AZIMUTHS {
            let r = interpolation::row(e, a);
            for ear in 0..2 {
                if a > 0 && interpolation::row(e, a - 1) != r {
                    connect(2 * r + ear, 2 * interpolation::row(e, a - 1) + ear);
                }
                if e > 0 {
                    connect(2 * r + ear, 2 * interpolation::row(e - 1, a) + ear);
                }
            }
        }
    }
    let mut d: Vec<_> = samples
        .as_chunks::<TAPS>()
        .0
        .iter()
        .map(|t| peak(t) as f64)
        .collect();
    for _ in 0..500 {
        for (i, neighbors) in edges.iter().enumerate() {
            if neighbors.is_empty() {
                continue;
            }
            d[i] = neighbors
                .iter()
                .map(|&(j, lag, w)| w * (d[j] + lag))
                .sum::<f64>()
                / neighbors.iter().map(|&(_, _, w)| w).sum::<f64>();
        }
    }
    (0..count)
        .map(|k| d[canonical(k)].round() as usize)
        .collect()
}

// For a mirrored model each signed pair must have opposite ITD and ILD.
fn mirror_floor(rows: &[Row], held_out: bool) {
    let mut itd = 0.0;
    let mut level = 0.0;
    let mut magnitude = 0.0;
    let basis = basis();
    for j in 0..12 {
        for i in 0..36 {
            let (a, e) = angles(i, j, held_out);
            let responses = [a, -a].map(|az| &nearest(rows, direction(az, e)).0.taps);
            let pair_itd = responses.map(|t| peak(&t[..TAPS]) - peak(&t[TAPS..]));
            itd += (0.5 * f64::from(pair_itd[0] + pair_itd[1])).powi(2);
            level += (0.5 * (ild(responses[0]) + ild(responses[1]))).powi(2);
            for ear in 0..2 {
                for wave in &basis {
                    let opposite = 1 - ear;
                    magnitude += (0.5
                        * (db(&responses[0][ear * TAPS..(ear + 1) * TAPS], wave)
                            - db(&responses[1][opposite * TAPS..(opposite + 1) * TAPS], wave)))
                    .powi(2);
                }
            }
        }
    }
    println!(
        "mirror RMS lower bounds held_out={held_out}: ITD {:.6}, ILD {:.6}, magnitude {:.6}",
        (itd / 432.0).sqrt(),
        (level / 432.0).sqrt(),
        (magnitude / (432.0 * 16.0)).sqrt()
    );
}

// Compare the model parameters before fractional filtering or block crossfades.
fn orbit_parameters(delays: &[f32], norms: &[f32], elevation: f32, smoothing: usize) {
    let mut last = None;
    let mut max_delay = 0.0f32;
    let mut max_level = 0.0f32;
    let mut worst = 0;
    for step in 0..=375 {
        let az = step as f32 * 360.0 / 375.0;
        let az = if az > 180.0 { az - 360.0 } else { az };
        let cell = interpolation::cell(az, elevation, smoothing);
        let ears = |values: &[f32]| {
            [0, 1].map(|ear| {
                interpolation::weighted_value(&cell, ear ^ usize::from(az < 0.0), values)
            })
        };
        let [dl, dr] = ears(delays);
        let [nl, nr] = ears(norms);
        let itd = dl - dr;
        let ild = 20.0 * (nl / nr).log10();
        if let Some((prev_itd, prev_ild)) = last {
            let dt: f32 = itd - prev_itd;
            let db: f32 = ild - prev_ild;
            max_delay = max_delay.max(dt.abs());
            if db.abs() > max_level {
                max_level = db.abs();
                worst = step - 1;
            }
        }
        last = Some((itd, ild));
    }
    println!("elevation={elevation} smoothsteps={smoothing} max blended ITD step={max_delay:.6} samples, norm-only max ILD step={max_level:.6} dB at interval {worst}");
}

fn main() -> Result<(), Box<dyn Error>> {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    if arguments.len() != 2 {
        return Err("usage: sadie-table INPUT.ojhrtf OUTPUT.bin".into());
    }
    let bytes = fs::read(&arguments[0])?;
    let digest = hex(&sha256(&bytes));
    if digest != INPUT_HASH {
        return Err(invalid("input does not match pinned SADIE II D1 SHA-256").into());
    }
    let rows = parse(&bytes)?;
    if rows.len() != MEASURED + 15 {
        return Err(invalid("expected 8802 measurements and 15 appended aliases").into());
    }
    println!(
        "input {} bytes, {} records; measured prefix {MEASURED}; SHA-256 {digest}",
        bytes.len(),
        rows.len()
    );
    let measured = &rows[..MEASURED];
    mirror_floor(measured, false);
    let old = Grid::new(measured, 10);
    let (mut table, samples) = encode(measured);
    let peaks: Vec<_> = samples
        .as_chunks::<TAPS>()
        .0
        .iter()
        .map(|t| peak(t) as usize)
        .collect();
    let early: Vec<_> = samples
        .as_chunks::<TAPS>()
        .0
        .iter()
        .map(|ear| {
            let energy = ear.iter().map(|t| t * t).sum::<f32>();
            let mut sum = 0.0;
            ear.iter()
                .position(|t| {
                    sum += t * t;
                    sum >= energy * 0.05
                })
                .unwrap()
        })
        .collect();
    let zero = vec![0; peaks.len()];
    evaluate("W1-A", measured, false, |a, e| old.at(a, e));
    for (name, delays) in [("raw", &zero), ("peak", &peaks), ("early", &early)] {
        evaluate(name, measured, false, |a, e| {
            interpolated(&samples, delays, a, e)
        });
    }
    let fit = fit_delays(&samples);
    let levels: Vec<_> = samples
        .as_chunks::<TAPS>()
        .0
        .iter()
        .map(|t| interpolation::level(t))
        .collect();
    let selected = |a: f64, e: f64, power| {
        let cell = interpolation::cell(a as f32, e as f32, power);
        let mut taps = [0.0; TAPS * 2];
        for (ear, out) in taps.as_chunks_mut::<TAPS>().0.iter_mut().enumerate() {
            let ch = ear ^ usize::from(a < 0.0);
            interpolation::blend(&cell, ch, &samples, &fit, out);
            interpolation::match_level(&cell, ch, &levels, out);
        }
        taps
    };
    let delay_values: Vec<_> = fit.iter().map(|&d| d as f32).collect();
    for (name, power) in [
        ("correlation", 0),
        ("correlation smoothstep", 1),
        ("correlation twice smoothstep", 2),
    ] {
        for elevation in [0.0, 30.0] {
            orbit_parameters(&delay_values, &levels, elevation, power);
        }
        for held_out in [false, true] {
            evaluate(name, measured, held_out, |a, e| selected(a, e, power));
        }
    }
    let mut legacy_max = 0.0f32;
    for e in 0..13 {
        for a in 0..19 {
            let az = (a * 10) as f64;
            let el = (e * 15) as f64 - 90.0;
            for (v, w) in selected(az, el, interpolation::SMOOTHSTEPS)
                .into_iter()
                .zip(old.at(az, el))
            {
                legacy_max = legacy_max.max((v - w).abs());
            }
        }
    }
    assert!(legacy_max < 0.0000145);
    println!("maximum change from 247 legacy nodes: {legacy_max:.12}");
    mirror_floor(measured, true);
    evaluate("W1-A", measured, true, |a, e| old.at(a, e));
    println!(
        "runtime uses {} smoothstep passes per axis",
        interpolation::SMOOTHSTEPS
    );
    for delay in fit {
        table.push(u8::try_from(delay)?);
    }
    fs::write(Path::new(&arguments[1]), &table)?;
    println!(
        "{} output bytes, SHA-256 {}",
        table.len(),
        hex(&sha256(&table))
    );
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

// FIPS 180-4 SHA-256, used only by this offline tool to reject a different source.
fn sha256(input: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut data = input.to_vec();
    data.push(0x80);
    while data.len() % 64 != 56 {
        data.push(0);
    }
    data.extend_from_slice(&((input.len() as u64) * 8).to_be_bytes());
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    for block in data.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for (word, bytes) in w.iter_mut().zip(block.as_chunks::<4>().0) {
            *word = u32::from_be_bytes(*bytes);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (word, add) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *word = word.wrapping_add(add);
        }
    }
    let mut result = [0; 32];
    for (bytes, word) in result.as_chunks_mut::<4>().0.iter_mut().zip(state) {
        bytes.copy_from_slice(&word.to_be_bytes());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_published_vectors() {
        assert_eq!(
            hex(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex(&sha256(&vec![b'a'; 1_000_000])),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    fn fixture() -> Vec<u8> {
        let mut payload = b"OJRIR2\0\0".to_vec();
        for value in [48_000, 1, TAPS as u32, 1, 1, 1, 1, 1] {
            payload.extend(value.to_le_bytes());
        }
        for value in [0.0f64, 1.0, 0.0] {
            payload.extend(value.to_le_bytes());
        }
        for value in [TAPS as u32, 0, 0] {
            payload.extend(value.to_le_bytes());
        }
        payload.extend(vec![0; TAPS * 2 * 4]);
        let mut bytes = b"OJHRTF2\0".to_vec();
        bytes.extend(2u32.to_le_bytes());
        bytes.extend(1u32.to_le_bytes());
        bytes.extend((payload.len() as u64).to_le_bytes());
        bytes.extend(sha256(&payload));
        bytes.extend(payload);
        bytes
    }

    #[test]
    fn parser_checks_integrity_and_record_shape() {
        let valid = fixture();
        assert_eq!(parse(&valid).unwrap().len(), 1);
        for length in [0, 7, 20, 55, 100, valid.len() - 1] {
            assert!(parse(&valid[..length]).is_err());
        }
        let mut corrupt = valid.clone();
        *corrupt.last_mut().unwrap() ^= 1;
        assert!(parse(&corrupt).is_err());
        let mut delay = valid.clone();
        delay[124..128].copy_from_slice(&1u32.to_le_bytes());
        let checksum = sha256(&delay[56..]);
        delay[24..56].copy_from_slice(&checksum);
        assert!(parse(&delay).is_err());
        let mut nonfinite = valid;
        nonfinite[132..136].copy_from_slice(&f32::NAN.to_le_bytes());
        let checksum = sha256(&nonfinite[56..]);
        nonfinite[24..56].copy_from_slice(&checksum);
        assert!(parse(&nonfinite).is_err());
    }

    #[test]
    fn coordinate_convention_is_right_front_up() {
        for (a, e, expected) in [
            (90.0, 0.0, [1.0, 0.0, 0.0]),
            (0.0, 0.0, [0.0, 1.0, 0.0]),
            (0.0, 90.0, [0.0, 0.0, 1.0]),
        ] {
            for (actual, wanted) in direction(a, e).iter().zip(expected) {
                assert!((actual - wanted).abs() < 1e-15);
            }
        }
    }
}
