//! Offline reduction of the pinned SADIE II D1 OpenJOC asset. No network or dependencies.

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

fn evaluate(grid: &Grid, rows: &[Row]) {
    let mut squared = 0.0;
    let mut reference_energy = 0.0;
    let mut max: f64 = 0.0;
    let mut itd_max = 0;
    let mut itd_squared = 0.0;
    let mut ild_max: f64 = 0.0;
    let mut ild_squared = 0.0;
    let frequencies = [
        200.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 12000.0, 16000.0,
    ];
    let basis: Vec<Vec<(f64, f64)>> = frequencies
        .iter()
        .map(|frequency| {
            (0..TAPS)
                .map(|tap| {
                    let angle = 2.0 * PI * frequency * tap as f64 / 48_000.0;
                    (angle.cos(), angle.sin())
                })
                .collect()
        })
        .collect();
    let mut spectral_max: f64 = 0.0;
    let mut spectral_squared = 0.0;
    let mut count = 0;
    // Common held-out directions for both grids. None is on either azimuth grid.
    for e in 0..12 {
        for a in 0..36 {
            for sign in [-1.0, 1.0] {
                let azimuth = (2.5 + a as f64 * 5.0) * sign;
                let elevation = -82.5 + e as f64 * 15.0;
                let (reference, _) = nearest(rows, direction(azimuth, elevation));
                let actual = grid.at(azimuth, elevation);
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
                        let magnitude = |taps: &[f32]| {
                            let (re, im) = taps.iter().zip(wave).fold(
                                (0.0, 0.0),
                                |(re, im), (&tap, &(c, s))| {
                                    (re + tap as f64 * c, im + tap as f64 * s)
                                },
                            );
                            20.0 * re.hypot(im).max(0.001).log10()
                        };
                        let range = ear * TAPS..(ear + 1) * TAPS;
                        let difference =
                            magnitude(&actual[range.clone()]) - magnitude(&reference.taps[range]);
                        spectral_max = spectral_max.max(difference.abs());
                        spectral_squared += difference * difference;
                    }
                }
                count += 1;
            }
        }
    }
    println!("grid {}x15: {count} held-out directions; tap max={max:.9}, RMS={:.9}, normalized RMS={:.9}; peak ITD max={itd_max} samples RMS={:.6}; energy ILD max={ild_max:.6} dB RMS={:.6} dB", grid.azimuth_step, (squared / (count * TAPS * 2) as f64).sqrt(), (squared / reference_energy).sqrt(), (itd_squared / count as f64).sqrt(), (ild_squared / count as f64).sqrt());
    println!("grid {}x15: magnitude error at {frequencies:?} Hz, -60dB absolute floor: max={spectral_max:.6} dB RMS={:.6} dB", grid.azimuth_step, (spectral_squared / (count * 2 * frequencies.len()) as f64).sqrt());
}

fn write_grid(grid: &Grid, output: &Path) -> io::Result<()> {
    let mut bytes = Vec::with_capacity(grid.rows.len() * TAPS * 2 * 4);
    let peak = grid
        .rows
        .iter()
        .flatten()
        .map(|x| x.abs())
        .fold(0.0, f32::max);
    let mut max: f64 = 0.0;
    let mut squared = 0.0;
    for &tap in grid.rows.iter().flatten() {
        bytes.extend_from_slice(&tap.to_le_bytes());
        let encoded = (tap as f64 / peak as f64 * 32767.0).round();
        let decoded = encoded * peak as f64 / 32767.0;
        let error = tap as f64 - decoded;
        max = max.max(error.abs());
        squared += error * error;
    }
    fs::write(output, &bytes)?;
    println!("selected f32: {} bytes, lossless f32 encode/decode; alternative globally scaled i16 peak={peak:.9} max error={max:.12} RMS={:.12}, {} bytes plus scale", bytes.len(), (squared / (bytes.len() / 4) as f64).sqrt(), bytes.len() / 2);
    println!("output SHA-256 {}", hex(&sha256(&bytes)));
    Ok(())
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
    let coarse = Grid::new(measured, 15);
    evaluate(&coarse, measured);
    let selected = Grid::new(measured, 10);
    evaluate(&selected, measured);
    let right = selected.at(30.0, 0.0);
    println!(
        "FR peak L={} R={}, energy ILD={:.6} dB",
        peak(&right[..TAPS]),
        peak(&right[TAPS..]),
        ild(&right)
    );
    write_grid(&selected, Path::new(&arguments[1]))?;
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
