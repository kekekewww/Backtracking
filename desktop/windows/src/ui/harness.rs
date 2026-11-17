//! A test copy of the app (see `startup::test_instance`) can put its window into a known
//! state, photograph it, write how long its frames took and quit. This lets a change to what
//! the window draws be checked without a person at the screen.
//!
//! - `BACKTRACKING_TEST_SCREENSHOT=<file.png>` turns it on.
//! - `BACKTRACKING_TEST_SCREENSHOT_DELAY_MS=<ms>` is how long the app runs first (default 4000).
//! - `BACKTRACKING_TEST_VIEW=<steps>` is done when the delay has passed, before the picture.
//!   Steps are separated by `;`. Times are seconds back from the newest audio:
//!   `zoom=<seconds>` shows only that much; `select=<from>-<to>` selects that stretch, as a
//!   drag would; `scroll=<lane>:<row>` scrolls a stacked lane (A is 0) to that row;
//!   `group=<lane>:<group id>` shows another channel group.
//!
//! The picture is `<file.png>`; `<file.png>.log` gets the frame times and any problem.

use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use eframe::egui;

use super::Recorder;

pub(super) const SCREENSHOT: &str = "BACKTRACKING_TEST_SCREENSHOT";
const SCREENSHOT_DELAY: &str = "BACKTRACKING_TEST_SCREENSHOT_DELAY_MS";
const VIEW: &str = "BACKTRACKING_TEST_VIEW";
/// Where a test copy puts its window, in physical pixels: `<x>,<y>`.
pub(super) const WINDOW_POSITION: &str = "BACKTRACKING_TEST_WINDOW_POSITION";
/// Frames drawn while the app starts are left out of the timings.
const WARM_UP: Duration = Duration::from_millis(1500);
/// Time for a step's effect (a new waveform, a frozen selection) to reach the screen.
const SETTLE: Duration = Duration::from_millis(500);

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Step {
    Zoom(f64),
    Select(f64, f64),
    Scroll(usize, usize),
    Group(usize, String),
}

pub(super) fn parse_steps(text: &str) -> Result<Vec<Step>, String> {
    text.split(';')
        .map(str::trim)
        .filter(|step| !step.is_empty())
        .map(|step| {
            let (name, value) = step
                .split_once('=')
                .ok_or_else(|| format!("no '=' in {step}"))?;
            let number = |text: &str| {
                text.trim()
                    .parse::<f64>()
                    .ok()
                    .filter(|value| value.is_finite() && *value >= 0.0)
                    .ok_or_else(|| format!("{text} is not a number of seconds"))
            };
            fn pair<'a>(step: &str, text: &'a str) -> Result<(&'a str, &'a str), String> {
                text.split_once(':')
                    .ok_or_else(|| format!("{step} needs <lane>:<value>"))
            }
            let lane = |text: &str| match text.trim() {
                "0" | "A" | "a" => Ok(0),
                "1" | "B" | "b" => Ok(1),
                other => Err(format!("{other} is not a lane")),
            };
            match name.trim() {
                "zoom" => Ok(Step::Zoom(number(value)?)),
                "select" => {
                    let (from, to) = value
                        .split_once('-')
                        .ok_or_else(|| format!("{step} needs <from>-<to>"))?;
                    Ok(Step::Select(number(from)?, number(to)?))
                }
                "scroll" => {
                    let (lane_text, row) = pair(step, value)?;
                    let row = row
                        .trim()
                        .parse()
                        .map_err(|_| format!("{row} is not a row"))?;
                    Ok(Step::Scroll(lane(lane_text)?, row))
                }
                "group" => {
                    let (lane_text, id) = pair(step, value)?;
                    Ok(Step::Group(lane(lane_text)?, id.trim().to_owned()))
                }
                other => Err(format!("unknown step {other}")),
            }
        })
        .collect()
}

pub(super) struct Harness {
    output: PathBuf,
    started: Instant,
    delay: Duration,
    steps: Vec<Step>,
    applied: Option<Instant>,
    result: Option<mpsc::Receiver<io::Result<[usize; 2]>>>,
    /// Seconds of work for each frame drawn after the warm-up, as eframe measured them.
    frame_seconds: Vec<f32>,
    first_timed: Option<Instant>,
    last_timed: Option<Instant>,
    notes: Vec<String>,
}

impl Harness {
    pub(super) fn from_environment() -> Option<Self> {
        let output = PathBuf::from(std::env::var_os(SCREENSHOT)?);
        let delay = std::env::var(SCREENSHOT_DELAY)
            .ok()
            .and_then(|value| value.trim().parse().ok())
            .map_or(Duration::from_millis(4000), Duration::from_millis);
        let mut notes = Vec::new();
        let steps = match std::env::var(VIEW) {
            Ok(text) => parse_steps(&text).unwrap_or_else(|error| {
                notes.push(format!("{VIEW} ignored: {error}"));
                Vec::new()
            }),
            Err(_) => Vec::new(),
        };
        Some(Self {
            output,
            started: Instant::now(),
            delay,
            steps,
            applied: None,
            result: None,
            frame_seconds: Vec::new(),
            first_timed: None,
            last_timed: None,
            notes,
        })
    }

    fn write_log(&self, picture: &io::Result<[usize; 2]>) {
        let mut log = String::new();
        match picture {
            Ok([width, height]) => {
                log.push_str(&format!(
                    "picture {} ({width} x {height} px)\n",
                    self.output.display()
                ));
            }
            Err(error) => log.push_str(&format!("picture failed: {error}\n")),
        }
        let mut seconds = self.frame_seconds.clone();
        seconds.sort_by(f32::total_cmp);
        if let (Some(first), Some(last)) = (self.first_timed, self.last_timed)
            && !seconds.is_empty()
        {
            let span = last.duration_since(first).as_secs_f64();
            let at = |fraction: f64| {
                seconds[((seconds.len() - 1) as f64 * fraction).round() as usize] * 1000.0
            };
            let mean = seconds.iter().map(|&value| f64::from(value)).sum::<f64>()
                / seconds.len() as f64
                * 1000.0;
            log.push_str(&format!(
                "frames {} in {span:.2} s ({:.1} per second)\n\
                 frame work ms: mean {mean:.2}, median {:.2}, 95th percentile {:.2}, max {:.2}\n",
                seconds.len(),
                if span > 0.0 {
                    (seconds.len() - 1) as f64 / span
                } else {
                    0.0
                },
                at(0.5),
                at(0.95),
                at(1.0),
            ));
        } else {
            log.push_str("no frames were timed\n");
        }
        for note in &self.notes {
            log.push_str(note);
            log.push('\n');
        }
        let mut path = self.output.clone().into_os_string();
        path.push(".log");
        if let Err(error) = fs::write(&path, log) {
            eprintln!("Could not write {}: {error}", PathBuf::from(path).display());
        }
    }
}

impl Recorder {
    /// Runs the test harness once per drawn frame, after everything else.
    pub(super) fn drive_harness(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let Some(harness) = self.harness.as_mut() else {
            return;
        };
        let now = Instant::now();
        let since_start = now.duration_since(harness.started);
        // Timing stops once the steps change what is drawn.
        if since_start >= WARM_UP
            && harness.applied.is_none()
            && let Some(seconds) = frame.info().cpu_usage
        {
            harness.frame_seconds.push(seconds);
            harness.first_timed.get_or_insert(now);
            harness.last_timed = Some(now);
        }
        if let Some(result) = &harness.result {
            match result.try_recv() {
                Ok(picture) => {
                    harness.write_log(&picture);
                    self.harness = None;
                    self.quit(ctx);
                }
                Err(mpsc::TryRecvError::Empty) => ctx.request_repaint_after(SETTLE),
                Err(mpsc::TryRecvError::Disconnected) => {
                    harness.write_log(&Err(io::Error::other("the screenshot was never taken")));
                    self.harness = None;
                    self.quit(ctx);
                }
            }
            return;
        }
        if since_start < harness.delay {
            // One wake at the end of the delay. It adds no frames to the ones measured.
            ctx.request_repaint_after(harness.delay - since_start);
            return;
        }
        let Some(applied) = harness.applied else {
            harness.applied = Some(now);
            let steps = std::mem::take(&mut harness.steps);
            for step in steps {
                if let Err(error) = self.apply_test_step(&step) {
                    if let Some(harness) = &mut self.harness {
                        harness.notes.push(format!("{step:?}: {error}"));
                    }
                }
            }
            ctx.request_repaint_after(SETTLE);
            return;
        };
        let settled = now.duration_since(applied) >= SETTLE
            && self.edit_request.is_none()
            && self.edit_preview.is_none();
        if !settled {
            ctx.request_repaint_after(Duration::from_millis(50));
            return;
        }
        let (send, receive) = mpsc::sync_channel(1);
        let output = harness.output.clone();
        let wake = ctx.clone();
        harness.result = Some(receive);
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(
            egui::ScreenshotCallback::new(move |image| {
                let size = image.size;
                let pixels: Vec<u8> = image
                    .pixels
                    .iter()
                    // The colours are premultiplied, so dropping alpha shows the window
                    // over black.
                    .flat_map(|pixel| [pixel.r(), pixel.g(), pixel.b()])
                    .collect();
                let written = fs::File::create(&output)
                    .and_then(|mut file| file.write_all(&png_rgb(size[0], size[1], &pixels)))
                    .map(|()| size);
                let _ = send.send(written);
                wake.request_repaint();
            }),
        ));
    }

    fn apply_test_step(&mut self, step: &Step) -> Result<(), String> {
        let rate = u64::from(self.sample_rate.max(1));
        let frames = |seconds: f64| (seconds * rate as f64).round() as u64;
        let history = self.history_frames.clone();
        if history.is_empty() {
            return Err("no audio yet".into());
        }
        match *step {
            Step::Zoom(seconds) => {
                let count = frames(seconds).max(1);
                self.set_visible_frames(history.end.saturating_sub(count), count);
            }
            Step::Select(from, to) => {
                let start = history
                    .end
                    .saturating_sub(frames(from.max(to)))
                    .max(history.start);
                let end = history
                    .end
                    .saturating_sub(frames(from.min(to)))
                    .max(start + 1);
                // As a finished drag does: freeze the audio, then keep the range.
                self.begin_editing();
                self.last_interaction = Instant::now();
                let flags = self.flag_update(start..end);
                self.apply_flags(flags);
                self.selection = Some(start..end);
                self.invalidate_export();
            }
            Step::Scroll(lane, row) => self.scroll_lane_to(lane, row),
            Step::Group(lane, ref id) => self.show_group(lane, id)?,
        }
        Ok(())
    }
}

/// Encodes 8-bit RGB pixels as a PNG. Each row takes whichever of the None, Sub and Up filters
/// leaves the smallest numbers, and the result is deflated with the fixed Huffman code and a
/// simple LZ77 search. A window picture is mostly flat colour, which this shrinks well.
pub(super) fn png_rgb(width: usize, height: usize, pixels: &[u8]) -> Vec<u8> {
    assert_eq!(pixels.len(), width * height * 3, "three bytes per pixel");
    let stride = width * 3;
    let mut filtered = Vec::with_capacity((stride + 1) * height);
    let mut candidates: [Vec<u8>; 3] = std::array::from_fn(|_| vec![0; stride]);
    for y in 0..height {
        let row = &pixels[y * stride..][..stride];
        let above = (y > 0).then(|| &pixels[(y - 1) * stride..][..stride]);
        for x in 0..stride {
            let left = if x >= 3 { row[x - 3] } else { 0 };
            let up = above.map_or(0, |above| above[x]);
            candidates[0][x] = row[x];
            candidates[1][x] = row[x].wrapping_sub(left);
            candidates[2][x] = row[x].wrapping_sub(up);
        }
        let cost = |bytes: &[u8]| {
            bytes
                .iter()
                .map(|&byte| u32::from((byte as i8).unsigned_abs()))
                .sum::<u32>()
        };
        let best = (0..3)
            .min_by_key(|&filter| cost(&candidates[filter]))
            .unwrap_or(0);
        filtered.push(best as u8);
        filtered.extend_from_slice(&candidates[best]);
    }

    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&(width as u32).to_be_bytes());
    header.extend_from_slice(&(height as u32).to_be_bytes());
    // Eight bits per sample, RGB, deflate, adaptive filtering, no interlace.
    header.extend_from_slice(&[8, 2, 0, 0, 0]);
    png_chunk(&mut png, b"IHDR", &header);
    png_chunk(&mut png, b"IDAT", &zlib(&filtered));
    png_chunk(&mut png, b"IEND", &[]);
    png
}

fn png_chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    png.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = png.len();
    png.extend_from_slice(kind);
    png.extend_from_slice(data);
    let crc = crc32(&png[start..]);
    png.extend_from_slice(&crc.to_be_bytes());
}

pub(super) fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

pub(super) fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in bytes.chunks(5552) {
        for &byte in chunk {
            a += u32::from(byte);
            b += a;
        }
        a %= 65_521;
        b %= 65_521;
    }
    (b << 16) | a
}

struct Bits {
    bytes: Vec<u8>,
    buffer: u64,
    count: u32,
}

impl Bits {
    /// Writes the low `count` bits of `value`, least significant first.
    fn put(&mut self, value: u32, count: u32) {
        self.buffer |= u64::from(value) << self.count;
        self.count += count;
        while self.count >= 8 {
            self.bytes.push(self.buffer as u8);
            self.buffer >>= 8;
            self.count -= 8;
        }
    }

    /// Writes a Huffman code, which deflate stores most significant bit first.
    fn code(&mut self, code: u32, length: u32) {
        self.put(code.reverse_bits() >> (32 - length), length);
    }

    fn literal(&mut self, symbol: u32) {
        match symbol {
            0..=143 => self.code(0x30 + symbol, 8),
            144..=255 => self.code(0x190 + symbol - 144, 9),
            256..=279 => self.code(symbol - 256, 7),
            _ => self.code(0xC0 + symbol - 280, 8),
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.count > 0 {
            self.bytes.push(self.buffer as u8);
        }
        self.bytes
    }
}

const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DISTANCE_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DISTANCE_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

/// A zlib stream holding one fixed-Huffman deflate block.
pub(super) fn zlib(data: &[u8]) -> Vec<u8> {
    const WINDOW: usize = 32_768;
    const HASH_BITS: u32 = 15;
    const CHAIN: usize = 24;
    let mut bits = Bits {
        bytes: vec![0x78, 0x01],
        buffer: 0,
        count: 0,
    };
    // The last block, compressed with the fixed codes.
    bits.put(1, 1);
    bits.put(1, 2);
    let hash = |at: usize| {
        let value =
            u32::from(data[at]) | u32::from(data[at + 1]) << 8 | u32::from(data[at + 2]) << 16;
        (value.wrapping_mul(0x9E37_79B1) >> (32 - HASH_BITS)) as usize
    };
    let mut head = vec![usize::MAX; 1 << HASH_BITS];
    let mut previous = vec![usize::MAX; data.len()];
    let insert = |at: usize, head: &mut Vec<usize>, previous: &mut Vec<usize>| {
        if at + 3 <= data.len() {
            let slot = hash(at);
            previous[at] = head[slot];
            head[slot] = at;
        }
    };
    let mut at = 0;
    while at < data.len() {
        let mut best = (0usize, 0usize);
        if at + 3 <= data.len() {
            let mut candidate = head[hash(at)];
            let longest = (data.len() - at).min(258);
            for _ in 0..CHAIN {
                if candidate == usize::MAX || at - candidate > WINDOW {
                    break;
                }
                let length = data[candidate..]
                    .iter()
                    .zip(&data[at..at + longest])
                    .take_while(|(a, b)| a == b)
                    .count();
                if length > best.0 {
                    best = (length, at - candidate);
                    if length == longest {
                        break;
                    }
                }
                candidate = previous[candidate];
            }
        }
        if best.0 >= 3 {
            let (length, distance) = best;
            let code = LENGTH_BASE
                .iter()
                .rposition(|&base| usize::from(base) <= length)
                .unwrap();
            bits.literal(257 + code as u32);
            bits.put(
                (length - usize::from(LENGTH_BASE[code])) as u32,
                u32::from(LENGTH_EXTRA[code]),
            );
            let code = DISTANCE_BASE
                .iter()
                .rposition(|&base| usize::from(base) <= distance)
                .unwrap();
            bits.code(code as u32, 5);
            bits.put(
                (distance - usize::from(DISTANCE_BASE[code])) as u32,
                u32::from(DISTANCE_EXTRA[code]),
            );
            for position in at..at + length {
                insert(position, &mut head, &mut previous);
            }
            at += length;
        } else {
            bits.literal(u32::from(data[at]));
            insert(at, &mut head, &mut previous);
            at += 1;
        }
    }
    bits.literal(256);
    let mut stream = bits.finish();
    stream.extend_from_slice(&adler32(data).to_be_bytes());
    stream
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads a fixed-Huffman deflate stream back, enough to check the encoder.
    fn inflate_fixed(stream: &[u8]) -> Vec<u8> {
        assert_eq!(&stream[..2], &[0x78, 0x01]);
        let body = &stream[2..stream.len() - 4];
        let mut position = 0usize;
        let mut bit = || {
            let value = (body[position / 8] >> (position % 8)) & 1;
            position += 1;
            u32::from(value)
        };
        let read = |count: u32, bit: &mut dyn FnMut() -> u32| {
            (0..count).fold(0, |value, index| value | bit() << index)
        };
        let code = |bit: &mut dyn FnMut() -> u32| {
            let mut value = 0;
            for length in 1..=9 {
                value = value << 1 | bit();
                let symbol = match length {
                    7 if value <= 0b001_0111 => Some(value + 256),
                    8 if (0x30..=0xBF).contains(&value) => Some(value - 0x30),
                    8 if (0xC0..=0xC7).contains(&value) => Some(value - 0xC0 + 280),
                    9 if value >= 0x190 => Some(value - 0x190 + 144),
                    _ => None,
                };
                if let Some(symbol) = symbol {
                    return symbol;
                }
            }
            panic!("not a fixed code");
        };
        assert_eq!(read(1, &mut bit), 1);
        assert_eq!(read(2, &mut bit), 1);
        let mut output = Vec::new();
        loop {
            let symbol = code(&mut bit);
            match symbol {
                0..=255 => output.push(symbol as u8),
                256 => break,
                _ => {
                    let index = (symbol - 257) as usize;
                    let length = usize::from(LENGTH_BASE[index])
                        + read(u32::from(LENGTH_EXTRA[index]), &mut bit) as usize;
                    let distance_code = (0..5).fold(0, |value, _| value << 1 | bit()) as usize;
                    let distance = usize::from(DISTANCE_BASE[distance_code])
                        + read(u32::from(DISTANCE_EXTRA[distance_code]), &mut bit) as usize;
                    for _ in 0..length {
                        output.push(output[output.len() - distance]);
                    }
                }
            }
        }
        let adler = u32::from_be_bytes(stream[stream.len() - 4..].try_into().unwrap());
        assert_eq!(adler, adler32(&output));
        output
    }

    #[test]
    fn checksums_match_their_published_check_values() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        assert_eq!(adler32(&[]), 1);
    }

    #[test]
    fn deflate_round_trips_runs_repeats_and_noise() {
        let mut noise = 0x1234_5678u32;
        let mut data: Vec<u8> = (0..40_000)
            .map(|index| match index / 10_000 {
                0 => 7,
                1 => (index % 13) as u8,
                _ => {
                    noise ^= noise << 13;
                    noise ^= noise >> 17;
                    noise ^= noise << 5;
                    noise as u8
                }
            })
            .collect();
        data.extend_from_slice(&data[..5000].to_vec());
        for input in [&data[..], &[][..], &[1, 2][..], &[9; 300][..]] {
            assert_eq!(inflate_fixed(&zlib(input)), input);
        }
        assert!(zlib(&[0; 100_000]).len() < 1000, "flat colour must shrink");
    }

    #[test]
    fn png_has_the_signature_header_and_checked_chunks() {
        let png = png_rgb(2, 1, &[255, 0, 0, 0, 0, 255]);
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(&png[16..24], &[0, 0, 0, 2, 0, 0, 0, 1]);
        assert_eq!(png[24..29], [8, 2, 0, 0, 0]);
        assert_eq!(&png[29..33], &crc32(&png[12..29]).to_be_bytes());
        let idat_length = u32::from_be_bytes(png[33..37].try_into().unwrap()) as usize;
        assert_eq!(&png[37..41], b"IDAT");
        let rows = inflate_fixed(&png[41..41 + idat_length]);
        // One filter byte, then the row: the Sub filter stores the second pixel as a difference.
        assert_eq!(rows.len(), 7);
        assert!(png.ends_with(&[0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xAE, 0x42, 0x60, 0x82]));
    }

    #[test]
    fn view_steps_are_read_in_order_and_bad_ones_are_named() {
        assert_eq!(
            parse_steps("zoom=2.5; select=1.5-0.25;scroll=A:4;group=b:objects").unwrap(),
            [
                Step::Zoom(2.5),
                Step::Select(1.5, 0.25),
                Step::Scroll(0, 4),
                Step::Group(1, "objects".into()),
            ]
        );
        assert_eq!(parse_steps(" ").unwrap(), []);
        for bad in ["zoom", "zoom=-1", "select=2", "scroll=C:1", "spin=1"] {
            assert!(parse_steps(bad).is_err(), "{bad}");
        }
    }
}
