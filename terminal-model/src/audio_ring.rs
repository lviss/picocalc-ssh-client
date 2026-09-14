//! Hardware-independent PCM sample ring buffer for push-to-talk capture.
//!
//! The microphone capture task and the network upload task must run
//! concurrently, and the firmware's heap is far too small (64 KiB, already
//! mostly taken by WiFi/TCP/SSH/SD) to buffer audio per chunk. This module
//! holds the fixed-capacity, allocation-free buffer they share:
//! [`AudioRing::write`] never blocks and never grows, and when the buffer is
//! full it discards the *oldest* samples to make room for the new ones. A
//! slow or unreachable destination therefore costs bounded audio, never a
//! stalled capture pipeline or an exhausted heap.
//!
//! Keeping this here (free of embassy/`crate` dependencies) lets host tests
//! exercise the exact code the firmware runs, per AGENTS.md's
//! "host-testable logic lives in terminal-model" guidance.

/// Outcome of an [`AudioRing::write`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteResult {
    /// How many of the oldest buffered samples were discarded to make room.
    pub dropped: usize,
    /// `true` only for the first overflowing write since the last
    /// [`AudioRing::clear`], so a caller can log each recording's first
    /// overflow exactly once instead of on every chunk.
    pub first_drop: bool,
}

/// A fixed-capacity ring of `i16` PCM samples.
///
/// `N` is the capacity in samples and must be greater than zero. The buffer
/// lives wherever the caller stores the value; the firmware pins it in
/// `.bss` as a `static`, so audio data never touches the heap.
pub struct AudioRing<const N: usize> {
    samples: [i16; N],
    /// Index of the oldest buffered sample.
    head: usize,
    /// Number of valid samples currently buffered (`<= N`).
    len: usize,
    /// Whether a drop has happened since the last `clear`, tracked so
    /// `first_drop` fires once per recording rather than once per overflow.
    dropped_since_clear: bool,
}

impl<const N: usize> AudioRing<N> {
    pub const fn new() -> Self {
        Self {
            samples: [0; N],
            head: 0,
            len: 0,
            dropped_since_clear: false,
        }
    }

    /// Empties the buffer and resets the first-drop indicator. Call this when
    /// a new recording begins so `first_drop` is reported once per recording.
    pub fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
        self.dropped_since_clear = false;
    }

    pub fn capacity(&self) -> usize {
        N
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Appends `samples`, discarding the oldest buffered samples when there
    /// isn't room. Never allocates, never blocks, and never grows past `N`.
    pub fn write(&mut self, samples: &[i16]) -> WriteResult {
        let mut dropped = 0;
        for &sample in samples {
            if self.len == N {
                self.head = (self.head + 1) % N;
                self.len -= 1;
                dropped += 1;
            }
            let tail = (self.head + self.len) % N;
            self.samples[tail] = sample;
            self.len += 1;
        }
        let first_drop = dropped > 0 && !self.dropped_since_clear;
        if dropped > 0 {
            self.dropped_since_clear = true;
        }
        WriteResult {
            dropped,
            first_drop,
        }
    }

    /// Removes up to `out.len()` of the oldest samples, writing them into
    /// `out`. Returns how many samples were written.
    pub fn read(&mut self, out: &mut [i16]) -> usize {
        let n = out.len().min(self.len);
        for (i, slot) in out.iter_mut().take(n).enumerate() {
            *slot = self.samples[(self.head + i) % N];
        }
        self.head = (self.head + n) % N;
        self.len -= n;
        n
    }
}

impl<const N: usize> Default for AudioRing<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_ring_is_empty() {
        let ring = AudioRing::<8>::new();
        assert_eq!(ring.capacity(), 8);
        assert_eq!(ring.len(), 0);
        assert!(ring.is_empty());
    }

    #[test]
    fn read_returns_samples_in_write_order() {
        let mut ring = AudioRing::<8>::new();
        ring.write(&[1, 2, 3]);
        let mut out = [0i16; 2];
        assert_eq!(ring.read(&mut out), 2);
        assert_eq!(out, [1, 2]);
        assert_eq!(ring.read(&mut out), 1);
        assert_eq!(out[0], 3);
        assert_eq!(ring.read(&mut out), 0);
        assert!(ring.is_empty());
    }

    #[test]
    fn read_reports_only_available_samples() {
        let mut ring = AudioRing::<8>::new();
        ring.write(&[7, 8]);
        let mut out = [0i16; 5];
        assert_eq!(ring.read(&mut out), 2);
        assert_eq!(out[..2], [7, 8]);
    }

    #[test]
    fn overflow_discards_oldest_samples_and_keeps_capacity() {
        let mut ring = AudioRing::<4>::new();
        let result = ring.write(&[1, 2, 3, 4, 5, 6]);
        assert_eq!(result.dropped, 2);
        assert_eq!(ring.len(), 4);
        assert_eq!(ring.capacity(), 4);
        let mut out = [0i16; 4];
        assert_eq!(ring.read(&mut out), 4);
        assert_eq!(out, [3, 4, 5, 6]);
    }

    #[test]
    fn write_larger_than_capacity_keeps_most_recent_samples() {
        let mut ring = AudioRing::<3>::new();
        let result = ring.write(&[1, 2, 3, 4, 5]);
        assert_eq!(result.dropped, 2);
        let mut out = [0i16; 3];
        assert_eq!(ring.read(&mut out), 3);
        assert_eq!(out, [3, 4, 5]);
    }

    #[test]
    fn exact_capacity_write_drops_nothing() {
        let mut ring = AudioRing::<4>::new();
        let result = ring.write(&[1, 2, 3, 4]);
        assert_eq!(result.dropped, 0);
        assert!(!result.first_drop);
        assert_eq!(ring.len(), 4);
    }

    /// The property the firmware relies on for its single log line: no matter
    /// how much data is pushed at a full ring, `first_drop` reports at most
    /// once per recording.
    #[test]
    fn first_drop_fires_exactly_once_per_recording() {
        let mut ring = AudioRing::<8>::new();
        let mut log_count = 0;
        for i in 0..100i16 {
            let result = ring.write(&[i, i + 1, i + 2, i + 3, i + 4]);
            if result.first_drop {
                log_count += 1;
            }
        }
        assert_eq!(log_count, 1);
        // A new recording resets the indicator, so it can fire again.
        ring.clear();
        assert!(ring.write(&[0; 20]).first_drop);
    }

    #[test]
    fn clear_empties_the_ring() {
        let mut ring = AudioRing::<4>::new();
        ring.write(&[1, 2, 3, 4]);
        ring.clear();
        assert!(ring.is_empty());
        let mut out = [0i16; 4];
        assert_eq!(ring.read(&mut out), 0);
    }

    /// A slow/unreachable destination can never make the buffer bigger than
    /// its capacity: whatever the write pattern, `len` stays bounded, which
    /// is what keeps memory usage from growing without bound.
    #[test]
    fn capacity_is_a_hard_upper_bound_under_arbitrary_writes() {
        let mut ring = AudioRing::<16>::new();
        let mut next = 0i16;
        let mut burst = [0i16; 100];
        for len in [1usize, 3, 7, 16, 64, 5, 100] {
            for sample in burst.iter_mut().take(len) {
                next = next.wrapping_add(1);
                *sample = next;
            }
            ring.write(&burst[..len]);
            assert!(ring.len() <= ring.capacity());
        }
        // After 100-sample pressure the buffer is full but still exactly N.
        assert_eq!(ring.len(), 16);
    }
}
