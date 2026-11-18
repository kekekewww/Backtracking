//! Object positions on the retained audio frame timeline.

use std::collections::VecDeque;
use std::mem::size_of;
use std::ops::Range;
use std::sync::Arc;

/// Maximum live storage per lane, including key pages and their indexes.
/// Frozen views can keep older shared pages alive, as frozen audio views do.
pub const MAX_POSITION_BYTES: usize = 8 * 1024 * 1024;
const CHANNELS: usize = 64;
const PAGE_KEYS: usize = 128;
const ARC_HEADER_BYTES: usize = 2 * size_of::<usize>();
const PAGE_BYTES: usize =
    size_of::<Page>() + PAGE_KEYS * size_of::<PositionKey>() + ARC_HEADER_BYTES;

/// Listener-relative position and unapplied linear gain.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Position {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub size: f64,
    pub gain: f64,
}

/// A position at an absolute frame on a lane's retained-audio timeline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PositionKey {
    pub frame: u64,
    pub position: Position,
}

struct Page {
    keys: Vec<PositionKey>,
    start: usize,
}

impl Page {
    fn new(keys: &[PositionKey]) -> Self {
        let mut storage = Vec::with_capacity(PAGE_KEYS);
        storage.extend_from_slice(keys);
        Self {
            keys: storage,
            start: 0,
        }
    }

    fn active(&self) -> &[PositionKey] {
        &self.keys[self.start..]
    }

    fn compact(&mut self) {
        if self.start != 0 {
            self.keys.drain(..self.start);
            self.start = 0;
        }
    }
}

impl Clone for Page {
    fn clone(&self) -> Self {
        Self::new(self.active())
    }
}

struct Storage {
    channels: [VecDeque<Arc<Page>>; CHANNELS],
    bytes: usize,
    len: usize,
}

impl Clone for Storage {
    fn clone(&self) -> Self {
        let channels = self.channels.clone();
        let bytes = size_of::<Self>()
            + ARC_HEADER_BYTES
            + channels
                .iter()
                .map(|pages| pages.capacity() * size_of::<Arc<Page>>() + pages.len() * PAGE_BYTES)
                .sum::<usize>();
        Self {
            channels,
            bytes,
            len: self.len,
        }
    }
}

impl Default for Storage {
    fn default() -> Self {
        Self {
            channels: std::array::from_fn(|_| VecDeque::new()),
            bytes: size_of::<Self>() + ARC_HEADER_BYTES,
            len: 0,
        }
    }
}

impl Storage {
    fn add_page(&mut self, channel: usize, index: usize, page: Page) {
        let pages = &mut self.channels[channel];
        let old_capacity = pages.capacity();
        pages.insert(index, Arc::new(page));
        self.bytes += PAGE_BYTES + (pages.capacity() - old_capacity) * size_of::<Arc<Page>>();
    }

    fn pop_oldest(&mut self, channel: usize) {
        let pages = &mut self.channels[channel];
        let page = Arc::make_mut(pages.front_mut().expect("nonempty channel"));
        page.start += 1;
        self.len -= 1;
        if page.active().is_empty() {
            pages.pop_front();
            self.bytes -= PAGE_BYTES;
            if pages.is_empty() {
                self.bytes -= pages.capacity() * size_of::<Arc<Page>>();
                *pages = VecDeque::new();
            }
        }
    }

    fn channel_positions(&self, channel: usize, frames: Range<u64>) -> Vec<PositionKey> {
        if frames.is_empty() {
            return Vec::new();
        }
        let Some(pages) = self.channels.get(channel) else {
            return Vec::new();
        };
        let mut previous = None;
        let mut result = Vec::new();
        for page in pages {
            let keys = page.active();
            if keys.last().expect("nonempty page").frame < frames.start {
                previous = keys.last().copied();
                continue;
            }
            for &key in keys {
                if key.frame < frames.start {
                    previous = Some(key);
                } else if key.frame < frames.end {
                    result.push(key);
                } else {
                    break;
                }
            }
            if keys.last().expect("nonempty page").frame >= frames.end {
                break;
            }
        }
        if let Some(previous) = previous {
            result.insert(0, previous);
        }
        result
    }
}

/// Mutable per-lane positions. Updates are ordered independently for each channel.
/// At the memory limit the oldest frame across all channels is removed first.
pub struct PositionTrack {
    data: Arc<Storage>,
    memory_limit: usize,
    overflow_pending: bool,
    overflow_reported: bool,
}

impl Default for PositionTrack {
    fn default() -> Self {
        Self::with_memory_limit(MAX_POSITION_BYTES)
    }
}

/// An immutable position view sharing its key pages with the live track.
#[derive(Clone)]
pub struct FrozenPositionTrack {
    data: Arc<Storage>,
}

impl PositionTrack {
    pub fn new() -> Self {
        Self::default()
    }

    /// Use a smaller live budget when the lane shares its allowance with queued
    /// positions. A limit below the empty index size is raised to that size.
    pub fn with_memory_limit(bytes: usize) -> Self {
        let data = Arc::new(Storage::default());
        Self {
            memory_limit: bytes.max(data.bytes),
            data,
            overflow_pending: false,
            overflow_reported: false,
        }
    }

    /// Insert or replace one channel's key. Channels outside 0..64 are ignored.
    pub fn insert(&mut self, channel: usize, key: PositionKey) {
        if channel >= CHANNELS {
            return;
        }
        let data = Arc::make_mut(&mut self.data);
        let pages = &data.channels[channel];
        if pages.is_empty()
            || pages.back().is_some_and(|page| {
                page.active().len() == PAGE_KEYS
                    && page.active().last().expect("nonempty page").frame < key.frame
            })
        {
            data.add_page(channel, pages.len(), Page::new(&[key]));
        } else {
            let pages = &data.channels[channel];
            let mut index = pages
                .iter()
                .position(|page| page.active().last().expect("nonempty page").frame >= key.frame)
                .unwrap_or(pages.len() - 1);
            let active = pages[index].active();
            if let Ok(offset) = active.binary_search_by_key(&key.frame, |key| key.frame) {
                let page = Arc::make_mut(&mut data.channels[channel][index]);
                page.keys[page.start + offset] = key;
                return;
            }
            if active.len() == PAGE_KEYS {
                // Split before inserting so a page's allocation never grows.
                let following = Page::new(&active[PAGE_KEYS / 2..]);
                let page = Arc::make_mut(&mut data.channels[channel][index]);
                page.compact();
                page.keys.truncate(PAGE_KEYS / 2);
                let belongs_after = key.frame > page.keys.last().expect("half page").frame;
                data.add_page(channel, index + 1, following);
                if belongs_after {
                    index += 1;
                }
            }
            let page = Arc::make_mut(&mut data.channels[channel][index]);
            page.compact();
            let offset = page.keys.partition_point(|stored| stored.frame < key.frame);
            page.keys.insert(offset, key);
        }
        data.len += 1;
        while data.bytes > self.memory_limit {
            let oldest = data
                .channels
                .iter()
                .enumerate()
                .filter_map(|(channel, pages)| {
                    pages.front().map(|page| (channel, page.active()[0].frame))
                })
                .min_by_key(|&(channel, frame)| (frame, channel));
            let Some((channel, _)) = oldest else {
                break;
            };
            data.pop_oldest(channel);
            if !self.overflow_reported {
                self.overflow_pending = true;
                self.overflow_reported = true;
            }
        }
    }

    /// Age out keys before audio's first frame, keeping one preceding key per
    /// channel. Future keys remain available until their audio arrives. An empty
    /// audio range clears the track, matching a cleared lane.
    pub fn retain_frames(&mut self, frames: Range<u64>) {
        if frames.is_empty() {
            self.clear();
            return;
        }
        let data = Arc::make_mut(&mut self.data);
        for channel in 0..CHANNELS {
            let before = data.channels[channel]
                .iter()
                .flat_map(|page| page.active())
                .take_while(|key| key.frame < frames.start)
                .count();
            for _ in 0..before.saturating_sub(1) {
                data.pop_oldest(channel);
            }
        }
    }

    /// Clear positions and reset the once-per-history overflow notification.
    pub fn clear(&mut self) {
        *self = Self::with_memory_limit(self.memory_limit);
    }

    /// Freeze without copying keys. Later writes copy only their touched pages.
    pub fn freeze(&self) -> FrozenPositionTrack {
        FrozenPositionTrack {
            data: self.data.clone(),
        }
    }

    /// Returns the pending memory-limit notice once until the next clear.
    pub fn take_overflowed(&mut self) -> bool {
        std::mem::take(&mut self.overflow_pending)
    }

    /// Bytes allocated by the live pages and indexes, excluding allocator headers
    /// and pages retained only by frozen views.
    pub fn memory_bytes(&self) -> usize {
        self.data.bytes
    }
    pub fn len(&self) -> usize {
        self.data.len
    }
    pub fn is_empty(&self) -> bool {
        self.data.len == 0
    }

    /// Read keys in the half-open range and the last key strictly before it.
    /// Before a channel's first key the result is empty until that first key.
    pub fn channel_positions(&self, channel: usize, frames: Range<u64>) -> Vec<PositionKey> {
        self.data.channel_positions(channel, frames)
    }
}

impl FrozenPositionTrack {
    pub fn channel_positions(&self, channel: usize, frames: Range<u64>) -> Vec<PositionKey> {
        self.data.channel_positions(channel, frames)
    }
    pub fn memory_bytes(&self) -> usize {
        self.data.bytes
    }
    pub fn len(&self) -> usize {
        self.data.len
    }
    pub fn is_empty(&self) -> bool {
        self.data.len == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(frame: u64) -> PositionKey {
        PositionKey {
            frame,
            position: Position {
                x: 0.5,
                y: 0.25,
                z: 0.,
                size: 0.,
                gain: 1.,
            },
        }
    }

    #[test]
    fn range_includes_predecessor_and_only_in_range_updates() {
        let mut track = PositionTrack::new();
        for frame in [10, 20, 30, 40] {
            track.insert(1, key(frame));
        }
        assert_eq!(track.channel_positions(1, 25..40), vec![key(20), key(30)]);
        assert_eq!(
            track.channel_positions(1, 20..40),
            vec![key(10), key(20), key(30)]
        );
        assert_eq!(track.channel_positions(1, 0..20), vec![key(10)]);
        assert!(track.channel_positions(0, 0..50).is_empty());
        assert!(track.channel_positions(1, 20..20).is_empty());
    }

    #[test]
    fn out_of_order_keys_and_replacement_stay_sorted_across_pages() {
        let mut track = PositionTrack::new();
        for frame in (0..400).rev() {
            track.insert(0, key(frame * 2));
        }
        for frame in 0..400 {
            track.insert(0, key(frame * 2 + 1));
        }
        let mut replacement = key(250);
        replacement.position.x = -1.;
        track.insert(0, replacement);
        let keys = track.channel_positions(0, 0..800);
        assert_eq!(keys.len(), 800);
        assert!(keys
            .iter()
            .enumerate()
            .all(|(i, key)| key.frame == i as u64));
        assert_eq!(keys[250], replacement);
        assert_eq!(track.len(), 800);
    }

    #[test]
    fn ageing_preserves_each_channel_predecessor_and_future_keys() {
        let mut track = PositionTrack::new();
        for channel in [0, 1] {
            for frame in 0..400 {
                track.insert(channel, key(frame));
            }
        }
        track.retain_frames(250..300);
        assert_eq!(track.len(), 302);
        assert_eq!(
            track.channel_positions(0, 250..252),
            vec![key(249), key(250), key(251)]
        );
        assert_eq!(
            track.channel_positions(1, 250..252),
            vec![key(249), key(250), key(251)]
        );
        assert_eq!(
            track.channel_positions(0, 399..401),
            vec![key(398), key(399)]
        );
        track.retain_frames(500..600);
        assert_eq!(track.len(), 2);
        assert_eq!(track.channel_positions(0, 500..600), vec![key(399)]);
    }

    #[test]
    fn frozen_keys_survive_live_edits_ageing_and_clear() {
        let mut track = PositionTrack::new();
        for frame in 0..400 {
            track.insert(1, key(frame));
        }
        let frozen = track.freeze();
        assert!(Arc::ptr_eq(&track.data, &frozen.data));
        let shared_page = track.data.channels[1][2].clone();
        track.insert(1, key(400));
        assert!(Arc::ptr_eq(&shared_page, &track.data.channels[1][2]));
        let mut changed = key(200);
        changed.position.gain = 0.5;
        track.insert(1, changed);
        assert_eq!(
            frozen.channel_positions(1, 200..201),
            vec![key(199), key(200)]
        );
        track.retain_frames(350..450);
        track.clear();
        assert!(track.is_empty());
        assert_eq!(
            frozen.channel_positions(1, 399..401),
            vec![key(398), key(399)]
        );
        assert_eq!(frozen.len(), 400);
    }

    #[test]
    fn repeated_freezes_do_not_charge_old_index_capacity_to_the_live_budget() {
        let mut track = PositionTrack::with_memory_limit(40_000);
        let mut snapshots = Vec::new();
        for frame in 0..500 {
            snapshots.push(track.freeze());
            track.insert(0, key(frame));
        }
        assert_eq!(track.len(), 500);
        assert!(!track.take_overflowed());
        assert_eq!(
            snapshots[250].channel_positions(0, 249..251),
            vec![key(248), key(249)]
        );
    }

    #[test]
    fn memory_limit_evicts_global_oldest_before_newer_channels_once() {
        let mut track = PositionTrack::with_memory_limit(18_000);
        for frame in 0..128 {
            track.insert(1, key(frame));
        }
        for frame in 128..256 {
            track.insert(0, key(frame));
        }
        track.insert(0, key(256));
        assert!(track.memory_bytes() <= 18_000);
        assert!(track.channel_positions(1, 0..1000).is_empty());
        assert_eq!(track.channel_positions(0, 0..1000).len(), 129);
        assert!(track.take_overflowed());
        assert!(!track.take_overflowed());
        for frame in 257..1024 {
            track.insert(0, key(frame));
        }
        assert!(!track.take_overflowed());
        assert!(track.memory_bytes() <= 18_000);
        track.clear();
        for frame in 0..1024 {
            track.insert(0, key(frame));
        }
        assert!(track.take_overflowed());
    }

    #[test]
    fn tiny_limit_and_empty_audio_clear_storage_without_panicking() {
        let mut track = PositionTrack::with_memory_limit(0);
        let empty_bytes = track.memory_bytes();
        track.insert(63, key(100));
        track.insert(64, key(100));
        assert!(track.is_empty());
        assert_eq!(track.memory_bytes(), empty_bytes);
        let mut track = PositionTrack::new();
        track.insert(0, key(10));
        track.retain_frames(10..10);
        assert!(track.is_empty());
    }

    #[test]
    fn ten_minutes_of_sixteen_objects_stays_within_eight_mib() {
        let mut track = PositionTrack::new();
        let start = std::time::Instant::now();
        for tick in 0..18_600_u64 {
            for channel in 0..16 {
                track.insert(channel, key(tick * 48_000 / 31));
            }
        }
        let elapsed = start.elapsed();
        assert!(track.memory_bytes() <= 8_388_608);
        assert!(track.take_overflowed());
        let first = track.channel_positions(0, 0..u64::MAX)[0].frame;
        println!("16 objects x 31 Hz x 600 s: input keys=297600, retained keys={}, live allocation bytes={}, first frame={}, elapsed={elapsed:?}, ns/input key={:.1}",
            track.len(), track.memory_bytes(), first, elapsed.as_nanos() as f64 / 297_600.);
    }
}
