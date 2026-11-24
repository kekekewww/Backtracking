//! Band energies per 128-frame block and channel, for colouring the waveform by frequency.
//!
//! Each channel runs through a tree of 4th-order Butterworth crossovers at the edges of
//! [`SPECTRUM_BAND_EDGES_HZ`], highest first: the high-pass side of a crossover is one band
//! and the low-pass side feeds the next crossover. Butterworth low and high passes at one
//! frequency are power complementary, so the bands split the audio's energy without gaps or
//! double counting. A 2nd-order high pass at 20 Hz removes DC and rumble from the lowest band.
//! Above 48 kHz a 4th-order low pass at 20 kHz comes first. Once a low pass has removed what
//! a lower sample rate cannot hold, the signal is decimated, so the low bands cost little: at
//! 48 kHz the whole tree costs about eight second-order sections per sample, and analysing
//! ten minutes of stereo takes about half a second. A block's value is the mean square of
//! each band over the block, smoothed by a one pole with a time constant of one period of
//! the band's geometric centre, so a low band does not flicker with the phase of its own
//! waveform. Energy confined to one band leaks less than 1 % into its neighbours once the
//! tone is an octave from the edge, and after the tilt pink noise reads within 5 % of flat.
//!
//! Every page is analysed on its own, from its own samples, so a block's value never
//! depends on which page came before, which range was read first or whether an older page
//! has already left the history. Filters that start at rest ring for a while, so the first
//! 50 ms of a page come from the same filters run backwards in time from 100 ms in, and the
//! forward filters, settled by then, take over from there to the end of the page.
//!
//! Reads build and extend the cache; appending audio never does. Only blocks whose samples
//! will not change are cached. The newest, incomplete block of a page that is still filling
//! takes the value of the block before it, and a page's first 100 ms are analysed afresh by
//! each read until all of them have arrived.

use super::*;
use crate::waveform::{BandEnergies, SpectrumBin, SPECTRUM_BANDS, SPECTRUM_BAND_EDGES_HZ};

const BANDS: usize = SPECTRUM_BANDS;
const BLOCK: usize = PEAK_FRAMES;
/// The lowest bands keep at least four samples per block.
const MAX_DECIMATION: usize = 32;
/// How much of the start of a page the backward pass covers.
const LEAD_SECONDS: f64 = 0.05;
/// Mean square per channel below which a bin has no colour: -90 dBFS.
const SILENT: f64 = 1e-9;
/// Larger finite samples are clipped, so one wild value cannot overflow a block's energy.
const MAX_SAMPLE: f32 = 1e3;
/// Butterworth sections: a 4th-order filter is one of each of the first two, in series.
const Q_FIRST: f64 = 0.541_196_100_146_197;
const Q_SECOND: f64 = 1.306_562_964_876_377;
const Q_SECOND_ORDER: f64 = std::f64::consts::FRAC_1_SQRT_2;

#[derive(Clone, Copy)]
struct Coefficients {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl Coefficients {
    /// A second-order low pass, or high pass, at `frequency` and `rate`: the bilinear
    /// transform of the analogue section, warped to keep its corner where it was.
    fn new(frequency: f64, rate: f64, q: f64, high: bool) -> Self {
        let omega = std::f64::consts::TAU * frequency / rate;
        let (sin, cos) = omega.sin_cos();
        let alpha = sin / (2.0 * q);
        let a0 = 1.0 + alpha;
        let (b0, b1) = if high {
            ((1.0 + cos) / 2.0, -(1.0 + cos))
        } else {
            ((1.0 - cos) / 2.0, 1.0 - cos)
        };
        Self {
            b0: (b0 / a0) as f32,
            b1: (b1 / a0) as f32,
            b2: (b0 / a0) as f32,
            a1: (-2.0 * cos / a0) as f32,
            a2: ((1.0 - alpha) / a0) as f32,
        }
    }

    /// A 4th-order Butterworth low pass or high pass, as two sections in series.
    fn butterworth(frequency: f64, rate: f64, high: bool) -> [Self; 2] {
        [Q_FIRST, Q_SECOND].map(|q| Self::new(frequency, rate, q, high))
    }
}

/// A second-order section in transposed direct form II, whose short feedback path lets
/// the processor overlap successive samples.
#[derive(Clone, Copy, Default)]
struct Filter {
    s1: f32,
    s2: f32,
}

impl Filter {
    #[inline(always)]
    fn tick(&mut self, c: &Coefficients, input: f32) -> f32 {
        let output = c.b0 * input + self.s1;
        self.s1 = c.b1 * input - c.a1 * output + self.s2;
        self.s2 = c.b2 * input - c.a2 * output;
        output
    }

    /// Two sections in series.
    #[inline(always)]
    fn pair(filters: &mut [Self; 2], c: &[Coefficients; 2], input: f32) -> f32 {
        let middle = filters[0].tick(&c[0], input);
        filters[1].tick(&c[1], middle)
    }

    /// Keeps a decaying state out of subnormal numbers, which are slow.
    fn flush(&mut self) {
        if self.s1.abs() < 1e-20 {
            self.s1 = 0.0;
        }
        if self.s2.abs() < 1e-20 {
            self.s2 = 0.0;
        }
    }
}

/// One crossover: its high-pass side is `band`, its low-pass side continues down the tree.
struct Stage {
    /// Decimation of the incoming signal, applied before this crossover.
    step: usize,
    band: usize,
    low: [Coefficients; 2],
    high: [Coefficients; 2],
}

/// The filter tree and weighting for one sample rate.
struct Design {
    rate: f64,
    front: Option<[Coefficients; 2]>,
    stages: Vec<Stage>,
    floor: Option<(usize, Coefficients)>,
    /// Gives pink noise equal tilted energies: the reciprocal of each band's width in octaves.
    tilt: [f32; BANDS],
    /// Each band's geometric centre in Hz and its smoothing factor for a whole block.
    centre: [f64; BANDS],
    smoothing: [f32; BANDS],
    lead: usize,
}

/// The decimation a signal low-passed at `cutoff` allows: the rate is halved while its new
/// Nyquist frequency stays at least 1.9 times the cutoff.
fn decimation(cutoff: f64, rate: &mut f64, total: &mut usize) -> usize {
    let mut step = 1;
    while *total * 2 <= MAX_DECIMATION && *rate / 2.0 >= 3.8 * cutoff {
        *rate /= 2.0;
        *total *= 2;
        step *= 2;
    }
    step
}

impl Design {
    fn new(sample_rate: u32) -> Self {
        let rate = f64::from(sample_rate);
        let nyquist = rate / 2.0;
        let usable = |edge: f64| edge * std::f64::consts::SQRT_2 <= nyquist;
        let edges = SPECTRUM_BAND_EDGES_HZ.map(f64::from);
        let mut current = rate;
        let mut total = 1;
        let (front, top, mut step) = if nyquist > 24_000.0 {
            let top = edges[BANDS];
            let front = Coefficients::butterworth(top, rate, false);
            (Some(front), top, decimation(top, &mut current, &mut total))
        } else {
            (None, nyquist, 1)
        };
        let mut stages = Vec::new();
        let mut upper = [0.0; BANDS];
        let mut above = top;
        for band in (1..BANDS).rev().filter(|&band| usable(edges[band])) {
            let edge = edges[band];
            stages.push(Stage {
                step,
                band,
                low: Coefficients::butterworth(edge, current, false),
                high: Coefficients::butterworth(edge, current, true),
            });
            upper[band] = above;
            above = edge;
            step = decimation(edge, &mut current, &mut total);
        }
        let floor = usable(edges[0]).then(|| {
            upper[0] = above;
            (
                step,
                Coefficients::new(edges[0], current, Q_SECOND_ORDER, true),
            )
        });
        let mut tilt = [0.0; BANDS];
        let mut centre = [0.0; BANDS];
        let mut smoothing = [0.0; BANDS];
        for band in 0..BANDS {
            if upper[band] > 0.0 {
                tilt[band] = (1.0 / (upper[band] / edges[band]).log2()) as f32;
                centre[band] = (upper[band] * edges[band]).sqrt();
                smoothing[band] = (-(BLOCK as f64) * centre[band] / rate).exp() as f32;
            }
        }
        Self {
            rate,
            front,
            stages,
            floor,
            tilt,
            centre,
            smoothing,
            lead: (LEAD_SECONDS * rate / BLOCK as f64).ceil() as usize * BLOCK,
        }
    }

    /// The frames at the start of a page whose blocks a backward pass analyses: whole
    /// blocks, at most half a page.
    fn lead(&self, page_frames: usize) -> usize {
        self.lead.min(page_frames / 2 / BLOCK * BLOCK)
    }

    /// Runs `frames` samples of `buffer` through `chain` and returns the smoothed mean
    /// square of each band. The buffer is used as scratch space. Without `smooth` the
    /// smoothing is left alone.
    fn block(
        &self,
        chain: &mut Chain,
        buffer: &mut [f32; BLOCK],
        frames: usize,
        smooth: bool,
    ) -> BandEnergies {
        #[cfg(test)]
        tests::ANALYSED.with(|count| count.set(count.get() + 1));
        let mut len = frames;
        let mut squares = [0.0f32; BANDS];
        if let Some(front) = &self.front {
            for sample in &mut buffer[..len] {
                *sample = Filter::pair(&mut chain.front, front, *sample);
            }
        }
        for (stage, [low, high]) in self.stages.iter().zip(&mut chain.stages) {
            len = decimate(buffer, len, stage.step);
            let mut energy = 0.0;
            for sample in &mut buffer[..len] {
                let band = Filter::pair(high, &stage.high, *sample);
                energy += band * band;
                *sample = Filter::pair(low, &stage.low, *sample);
            }
            squares[stage.band] = energy / len as f32;
        }
        if let Some((step, floor)) = &self.floor {
            len = decimate(buffer, len, *step);
            let mut energy = 0.0;
            for &sample in &buffer[..len] {
                let band = chain.floor.tick(floor, sample);
                energy += band * band;
            }
            squares[0] = energy / len as f32;
        }
        if !smooth {
            chain.flush();
            return squares;
        }
        // A running mean until there are enough blocks for the time constant.
        let mean = chain.smoothed_blocks as f32 / (chain.smoothed_blocks as f32 + 1.0);
        chain.smoothed_blocks = chain.smoothed_blocks.saturating_add(1);
        for band in 0..BANDS {
            let keep = if frames == BLOCK {
                self.smoothing[band]
            } else {
                (-(frames as f64) * self.centre[band] / self.rate).exp() as f32
            }
            .min(mean);
            let value = keep * chain.smoothed[band] + (1.0 - keep) * squares[band];
            chain.smoothed[band] = if value < 1e-30 { 0.0 } else { value };
        }
        chain.flush();
        chain.smoothed
    }

    /// The first `lead / BLOCK` blocks of `page` and the forward filters after them.
    ///
    /// Filters starting at rest ring for a while, so those blocks come from filters run
    /// backwards in time, from frame `back_from` down to the page's first frame: a band's
    /// energy is the same either way, and by then the backward filters have settled. The
    /// forward filters meanwhile settle over the same blocks, then take over with the
    /// smoothing where the backward pass left it.
    fn lead_in(
        &self,
        page: &Page,
        channels: usize,
        channel: usize,
        lead: usize,
        back_from: usize,
    ) -> (Vec<BandEnergies>, Chain) {
        let read = |frames: Range<usize>, buffer: &mut [f32; BLOCK], reverse: bool| {
            let samples = page
                .samples(frames.clone(), channels)
                .chunks_exact(channels);
            let count = frames.len();
            for (offset, frame) in samples.enumerate() {
                let index = if reverse { count - 1 - offset } else { offset };
                buffer[index] = clean(frame[channel]);
            }
            count
        };
        let mut buffer = [0.0; BLOCK];
        let mut backward = Chain::default();
        let mut lead_blocks = vec![[0.0; BANDS]; lead / BLOCK];
        for block in (0..back_from.div_ceil(BLOCK)).rev() {
            let frames = block * BLOCK..back_from.min((block + 1) * BLOCK);
            let count = read(frames, &mut buffer, true);
            let energies = self.block(&mut backward, &mut buffer, count, true);
            if let Some(slot) = lead_blocks.get_mut(block) {
                *slot = energies;
            }
        }
        let mut forward = Chain::default();
        for block in 0..lead / BLOCK {
            let count = read(block * BLOCK..(block + 1) * BLOCK, &mut buffer, false);
            self.block(&mut forward, &mut buffer, count, false);
        }
        if let Some(&last) = lead_blocks.last() {
            forward.smoothed = last;
            forward.smoothed_blocks = u16::MAX;
        }
        (lead_blocks, forward)
    }

    /// Analyses `block` of the first `frames` frames of `page`, continuing `chain`.
    fn analyse(
        &self,
        chain: &mut Chain,
        page: &Page,
        channels: usize,
        channel: usize,
        block: usize,
        frames: usize,
    ) -> BandEnergies {
        let mut buffer = [0.0; BLOCK];
        let first = block * BLOCK;
        let count = (frames - first).min(BLOCK);
        let samples = &page.samples[first * channels..(first + count) * channels];
        for (sample, frame) in buffer.iter_mut().zip(samples.chunks_exact(channels)) {
            *sample = clean(frame[channel]);
        }
        self.block(chain, &mut buffer, count, true)
    }

    /// Every block of the first `frames` frames of `page`, analysed from scratch and not
    /// cached, for a page that does not yet hold what [`Self::lead_in`] needs.
    fn uncached(
        &self,
        page: &Page,
        layout: Layout,
        channel: usize,
        frames: usize,
    ) -> Vec<BandEnergies> {
        let channels = layout.channels;
        let lead = self
            .lead(layout.page_frames)
            .min(frames / 2 / BLOCK * BLOCK);
        let (mut blocks, mut chain) = self.lead_in(page, channels, channel, lead, frames);
        for block in lead / BLOCK..frames.div_ceil(BLOCK) {
            blocks.push(self.analyse(&mut chain, page, channels, channel, block, frames));
        }
        blocks
    }
}

fn clean(sample: f32) -> f32 {
    if sample.is_finite() {
        sample.clamp(-MAX_SAMPLE, MAX_SAMPLE)
    } else {
        0.0
    }
}

/// Keeps every `step`th of the first `len` samples, starting with the first.
fn decimate(buffer: &mut [f32; BLOCK], len: usize, step: usize) -> usize {
    if step == 1 {
        return len;
    }
    let kept = len.div_ceil(step);
    for index in 1..kept {
        buffer[index] = buffer[index * step];
    }
    kept
}

/// The state of one channel's filters and smoothing between blocks.
#[derive(Clone, Default)]
struct Chain {
    front: [Filter; 2],
    stages: [[[Filter; 2]; 2]; BANDS - 1],
    floor: Filter,
    smoothed: BandEnergies,
    smoothed_blocks: u16,
}

impl Chain {
    fn flush(&mut self) {
        for filter in self
            .front
            .iter_mut()
            .chain(self.stages.iter_mut().flatten().flatten())
            .chain([&mut self.floor])
        {
            filter.flush();
        }
    }
}

#[derive(Default)]
struct ChannelBands {
    /// Untilted band energies of the page's first blocks.
    blocks: Vec<BandEnergies>,
    /// The filters after the last cached block, until the page is complete.
    chain: Option<Box<Chain>>,
}

/// One page's band energies, per channel. A frozen copy of a page that is still filling
/// shares it, so either one extends what the other has built; samples they both hold are
/// identical, and so is every block either one caches.
#[derive(Default)]
pub(super) struct BandCache {
    channels: OnceLock<Box<[Mutex<ChannelBands>]>>,
}

impl Page {
    /// The samples are about to change. A cache shared with a frozen copy stays with it.
    pub(super) fn invalidate_bands(&mut self) {
        match Arc::get_mut(&mut self.bands) {
            Some(cache) => {
                if let Some(channels) = cache.channels.get_mut() {
                    for channel in channels.iter_mut() {
                        let bands = channel.get_mut().unwrap_or_else(|error| error.into_inner());
                        bands.blocks.clear();
                        bands.chain = None;
                    }
                }
            }
            None => self.bands = Arc::default(),
        }
    }

    /// Calls `read` with the band energies of each block of `channel` that this page holds:
    /// first those from the cache, which this call extends, then any provisional ones.
    fn band_blocks<R>(
        &self,
        design: &Design,
        layout: Layout,
        channel: usize,
        read: impl FnOnce(&[BandEnergies], &[BandEnergies]) -> R,
    ) -> R {
        let channels = layout.channels;
        let frames = self.frames(channels);
        let blocks = frames.div_ceil(BLOCK);
        let lead = design.lead(layout.page_frames);
        // A page holding all of its frames caches its partial last block too.
        let complete = frames >= layout.page_frames;
        let lasting = if complete { blocks } else { frames / BLOCK };
        if frames < 2 * lead || lasting == 0 {
            return read(&[], &design.uncached(self, layout, channel, frames));
        }
        let slots = self.bands.channels.get_or_init(|| {
            (0..channels)
                .map(|_| Mutex::new(ChannelBands::default()))
                .collect()
        });
        let mut slot = slots[channel]
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if slot.blocks.len() < lasting {
            let ChannelBands {
                blocks: cached,
                chain,
            } = &mut *slot;
            if cached.capacity() == 0 {
                let capacity = layout.page_frames.min(self.samples.capacity() / channels);
                cached.reserve_exact(capacity.div_ceil(BLOCK).max(lasting));
            }
            debug_assert!(cached.is_empty() || chain.is_some());
            if cached.is_empty() {
                let (lead_blocks, state) = design.lead_in(self, channels, channel, lead, 2 * lead);
                cached.extend(lead_blocks);
                *chain = Some(Box::new(state));
            }
            let state = chain
                .as_mut()
                .expect("a page being extended keeps its filters");
            for block in cached.len()..lasting {
                cached.push(design.analyse(state, self, channels, channel, block, frames));
            }
            if complete {
                *chain = None;
            }
        }
        let cached = &slot.blocks[..lasting];
        let newest = [cached[lasting - 1]];
        read(cached, &newest[..blocks - lasting])
    }
}

/// Writes the tilted band energies of the channels in `members`, each scaled by its weight,
/// for each chronological bin of `range`, and returns its frame count. `pages` must hold
/// all of `range`. Bins are those of the waveform calls; a bin narrower than a block takes
/// the block's value, a wider one the frame-weighted mean of the blocks it overlaps.
pub(in crate::capture) fn bins<'a>(
    pages: impl Iterator<Item = (&'a Page, Range<usize>)>,
    layout: Layout,
    sample_rate: u32,
    members: &[(usize, f32)],
    range: Range<u64>,
    bins: &mut [SpectrumBin],
) -> usize {
    bins.fill(None);
    let count = bins.len();
    let frame_count = range.end.saturating_sub(range.start);
    if frame_count == 0 || count == 0 || members.is_empty() {
        return frame_count as usize;
    }
    let design = Design::new(sample_rate);
    let bin = |index: usize| {
        let at =
            |bin: usize| range.start + (bin as u128 * frame_count as u128 / count as u128) as u64;
        let start = at(index);
        start..at(index + 1).max(start + 1)
    };
    let mut sums = vec![[0.0f64; BANDS]; count];
    let mut first = 0;
    for (page, held) in pages {
        if held.is_empty() {
            continue;
        }
        let base = page.first_frame;
        let (start, end) = (base + held.start as u64, base + held.end as u64);
        while first < count && bin(first).end <= start {
            first += 1;
        }
        for &(channel, weight) in members {
            page.band_blocks(&design, layout, channel, |cached, provisional| {
                let block = |index: usize| match cached.get(index) {
                    Some(energies) => energies,
                    None => &provisional[index - cached.len()],
                };
                for (index, sum) in sums.iter_mut().enumerate().skip(first) {
                    let frames = bin(index);
                    if frames.start >= end {
                        break;
                    }
                    let from = (frames.start.max(start) - base) as usize;
                    let to = (frames.end.min(end) - base) as usize;
                    for index in from / BLOCK..to.div_ceil(BLOCK) {
                        let overlap = to.min((index + 1) * BLOCK) - from.max(index * BLOCK);
                        let scale = overlap as f64 * f64::from(weight);
                        for (sum, &energy) in sum.iter_mut().zip(block(index)) {
                            *sum += f64::from(energy) * scale;
                        }
                    }
                }
            });
        }
    }
    let silent = SILENT
        * members
            .iter()
            .map(|&(_, weight)| f64::from(weight))
            .sum::<f64>();
    for (index, (bin_out, sum)) in bins.iter_mut().zip(&sums).enumerate() {
        let frames = bin(index);
        let frames = (frames.end - frames.start) as f64;
        if sum.iter().sum::<f64>() / frames >= silent {
            let mut energies = [0.0; BANDS];
            for ((energy, &sum), &tilt) in energies.iter_mut().zip(sum).zip(&design.tilt) {
                *energy = (sum / frames) as f32 * tilt;
            }
            *bin_out = Some(energies);
        }
    }
    frame_count as usize
}

#[cfg(test)]
mod tests;
