use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

/// Bounded, nonblocking audio transfer between exactly one producer and one consumer.
/// All capacities and results are measured in frames.
pub struct AudioQueue {
    samples: Box<[AtomicU32]>,
    capacity: usize,
    channels: usize,
    read: AtomicUsize,
    write: AtomicUsize,
}

impl AudioQueue {
    pub fn new(capacity_frames: usize, channels: usize) -> Arc<Self> {
        assert!(capacity_frames > 0 && (1..=2).contains(&channels));
        Self::allocate(capacity_frames, channels)
    }

    fn allocate(capacity_frames: usize, channels: usize) -> Arc<Self> {
        let capacity = capacity_frames.next_power_of_two();
        Arc::new(Self {
            samples: (0..capacity * channels)
                .map(|_| AtomicU32::new(0))
                .collect(),
            capacity,
            channels,
            read: AtomicUsize::new(0),
            write: AtomicUsize::new(0),
        })
    }

    pub fn push_interleaved(&self, samples: &[f32]) -> usize {
        if samples.len() % self.channels != 0 {
            return 0;
        }
        self.push(samples.len() / self.channels, |frame, channel| {
            samples[frame * self.channels + channel]
        })
    }

    pub fn push_planar(&self, left: &[f32], right: Option<&[f32]>) -> usize {
        match (self.channels, right) {
            (1, None) => self.push(left.len(), |frame, _| left[frame]),
            (2, Some(right)) if right.len() == left.len() => {
                self.push(left.len(), |frame, channel| {
                    if channel == 0 {
                        left[frame]
                    } else {
                        right[frame]
                    }
                })
            }
            _ => 0,
        }
    }

    fn push(&self, frames: usize, sample: impl Fn(usize, usize) -> f32) -> usize {
        let write = self.write.load(Ordering::Relaxed);
        let read = self.read.load(Ordering::Acquire);
        let count = frames.min(self.capacity - write.wrapping_sub(read));

        for frame in 0..count {
            let offset = (write.wrapping_add(frame) & (self.capacity - 1)) * self.channels;
            for channel in 0..self.channels {
                self.samples[offset + channel]
                    .store(sample(frame, channel).to_bits(), Ordering::Relaxed);
            }
        }

        self.write
            .store(write.wrapping_add(count), Ordering::Release);
        count
    }

    /// The number of already published frames. Call only from the designated consumer.
    pub fn pending_frames(&self) -> usize {
        let read = self.read.load(Ordering::Relaxed);
        self.write.load(Ordering::Acquire).wrapping_sub(read)
    }

    pub fn pop_interleaved(&self, destination: &mut [f32]) -> usize {
        let read = self.read.load(Ordering::Relaxed);
        let write = self.write.load(Ordering::Acquire);
        let count = (destination.len() / self.channels).min(write.wrapping_sub(read));

        for (frame, destination) in destination
            .chunks_exact_mut(self.channels)
            .take(count)
            .enumerate()
        {
            let offset = (read.wrapping_add(frame) & (self.capacity - 1)) * self.channels;
            for (channel, sample) in destination.iter_mut().enumerate() {
                *sample = f32::from_bits(self.samples[offset + channel].load(Ordering::Relaxed));
            }
        }

        self.read.store(read.wrapping_add(count), Ordering::Release);
        count
    }

    /// Drops all published frames without copying them. Call only from the
    /// designated consumer. A producer block published later remains queued.
    pub fn discard_pending(&self) {
        let write = self.write.load(Ordering::Acquire);
        self.read.store(write, Ordering::Release);
    }
}

/// A complete or partially accepted packet and its first-frame host timestamp.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimedPacket {
    pub timestamp_ns: u64,
    pub sample_rate: u32,
    pub frames: usize,
}

struct PacketSlot {
    timestamp_ns: AtomicU64,
    sample_rate: AtomicU32,
    frames: AtomicUsize,
}

/// Bounded timestamped transfer between one audio producer and one consumer.
/// Publishing and copying never allocates or waits in the producer.
pub struct TimedAudioQueue {
    audio: Arc<AudioQueue>,
    packets: Box<[PacketSlot]>,
    channels: usize,
    read: AtomicUsize,
    write: AtomicUsize,
}

impl TimedAudioQueue {
    pub fn new(capacity_frames: usize, capacity_packets: usize, channels: usize) -> Arc<Self> {
        assert!(capacity_frames > 0 && capacity_packets > 0 && (1..=32).contains(&channels));
        Arc::new(Self {
            audio: AudioQueue::allocate(capacity_frames, channels),
            packets: (0..capacity_packets.next_power_of_two())
                .map(|_| PacketSlot {
                    timestamp_ns: AtomicU64::new(0),
                    sample_rate: AtomicU32::new(0),
                    frames: AtomicUsize::new(0),
                })
                .collect(),
            channels,
            read: AtomicUsize::new(0),
            write: AtomicUsize::new(0),
        })
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn push_interleaved(&self, timestamp_ns: u64, sample_rate: u32, samples: &[f32]) -> usize {
        self.push(timestamp_ns, sample_rate, || {
            self.audio.push_interleaved(samples)
        })
    }

    pub fn push_planar(
        &self,
        timestamp_ns: u64,
        sample_rate: u32,
        left: &[f32],
        right: Option<&[f32]>,
    ) -> usize {
        self.push(timestamp_ns, sample_rate, || {
            self.audio.push_planar(left, right)
        })
    }

    fn push(
        &self,
        timestamp_ns: u64,
        sample_rate: u32,
        write_audio: impl FnOnce() -> usize,
    ) -> usize {
        let write = self.write.load(Ordering::Relaxed);
        let read = self.read.load(Ordering::Acquire);
        if sample_rate == 0 || write.wrapping_sub(read) >= self.packets.len() {
            return 0;
        }
        let frames = write_audio();
        if frames == 0 {
            return 0;
        }
        let slot = &self.packets[write & (self.packets.len() - 1)];
        slot.timestamp_ns.store(timestamp_ns, Ordering::Relaxed);
        slot.sample_rate.store(sample_rate, Ordering::Relaxed);
        slot.frames.store(frames, Ordering::Relaxed);
        self.write.store(write.wrapping_add(1), Ordering::Release);
        frames
    }

    /// Resizes the consumer-owned scratch vector to hold exactly one packet.
    pub fn pop(&self, scratch: &mut Vec<f32>) -> Option<TimedPacket> {
        let read = self.read.load(Ordering::Relaxed);
        if read == self.write.load(Ordering::Acquire) {
            return None;
        }
        let slot = &self.packets[read & (self.packets.len() - 1)];
        let timestamp_ns = slot.timestamp_ns.load(Ordering::Relaxed);
        let sample_rate = slot.sample_rate.load(Ordering::Relaxed);
        let frames = slot.frames.load(Ordering::Relaxed);
        scratch.resize(frames * self.channels, 0.);
        let copied = self.audio.pop_interleaved(scratch);
        debug_assert_eq!(copied, frames);
        self.read.store(read.wrapping_add(1), Ordering::Release);
        Some(TimedPacket {
            timestamp_ns,
            sample_rate,
            frames,
        })
    }

    /// Number of already published packets. Call only from the consumer.
    pub fn pending_packets(&self) -> usize {
        let read = self.read.load(Ordering::Relaxed);
        self.write.load(Ordering::Acquire).wrapping_sub(read)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timed_queue_keeps_packet_metadata_and_native_layout_through_overflow() {
        let queue = TimedAudioQueue::new(4, 2, 3);
        let first = [0.25, 0.5, 0.75, -0.25, -0.5, -0.75];
        assert_eq!(queue.push_interleaved(90, 48_000, &first), 2);
        assert_eq!(queue.push_interleaved(120, 44_100, &[1., 2., 3.]), 1);
        assert_eq!(queue.push_interleaved(150, 44_100, &[1., 2., 3.]), 0);
        assert_eq!(queue.pending_packets(), 2);
        let mut samples = Vec::new();
        assert_eq!(
            queue.pop(&mut samples),
            Some(TimedPacket {
                timestamp_ns: 90,
                sample_rate: 48_000,
                frames: 2
            })
        );
        assert_eq!(samples, first);
        assert_eq!(queue.push_interleaved(170, 44_100, &[4.; 12]), 3);
        assert_eq!(queue.pop(&mut samples).unwrap().timestamp_ns, 120);
        assert_eq!(samples, [1., 2., 3.]);
        assert_eq!(
            queue.pop(&mut samples),
            Some(TimedPacket {
                timestamp_ns: 170,
                sample_rate: 44_100,
                frames: 3
            })
        );
        assert_eq!(samples, [4.; 9]);
        assert!(queue.pop(&mut samples).is_none());
    }

    #[test]
    fn planar_and_interleaved_keep_frame_order_across_wrap() {
        let queue = AudioQueue::new(2, 2);
        assert_eq!(queue.push_planar(&[1., 2.], Some(&[11., 12.])), 2);
        assert_eq!(queue.pending_frames(), 2);
        assert_eq!(queue.push_interleaved(&[3., 13.]), 0);
        let mut one = [0.; 2];
        assert_eq!(queue.pop_interleaved(&mut one), 1);
        assert_eq!(one, [1., 11.]);
        assert_eq!(queue.pending_frames(), 1);
        assert_eq!(queue.push_interleaved(&[3., 13., 4., 14.]), 1);
        let mut two = [0.; 4];
        assert_eq!(queue.pop_interleaved(&mut two), 2);
        assert_eq!(two, [2., 12., 3., 13.]);
        assert_eq!(queue.pop_interleaved(&mut two), 0);
        assert_eq!(queue.pending_frames(), 0);
    }

    #[test]
    fn producer_and_consumer_transfer_all_samples_in_order() {
        let queue = AudioQueue::new(8, 1);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                for value in 0..2048 {
                    while queue.push_interleaved(&[value as f32]) == 0 {
                        std::thread::yield_now();
                    }
                }
            });
            for value in 0..2048 {
                let mut sample = [0.];
                while queue.pop_interleaved(&mut sample) == 0 {
                    std::thread::yield_now();
                }
                assert_eq!(sample[0], value as f32);
            }
        });
    }
}
