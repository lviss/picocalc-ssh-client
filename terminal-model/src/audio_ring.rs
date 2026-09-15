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
//! Every sample is tagged with the *generation* (utterance) that produced
//! it. Utterances are serialized on the capture side, but a second recording
//! can begin while the first one's connection is still being established, so
//! their samples share this buffer. The generation tags let the upload task
//! drain exactly the audio belonging to the connection it currently holds
//! and never send a later utterance's audio (or consume its end) on an
//! earlier utterance's socket. [`utterance_ended`] is the matching pure
//! predicate for deciding when a generation has finished.
//!
//! Keeping this here (free of embassy/`crate` dependencies) lets host tests
//! exercise the exact code the firmware runs, per AGENTS.md's
//! "host-testable logic lives in terminal-model" guidance.

/// Outcome of an [`AudioRing::write`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteResult {
    /// How many of the oldest buffered samples were discarded to make room.
    pub dropped: usize,
    /// `true` only for the first overflowing write of a given generation, so
    /// a caller can log each recording's first overflow exactly once instead
    /// of on every chunk.
    pub first_drop: bool,
}

/// A fixed-capacity ring of `i16` PCM samples, each tagged with the utterance
/// generation that produced it.
///
/// `N` is the capacity in samples and must be greater than zero. The buffer
/// lives wherever the caller stores the value; the firmware pins it in
/// `.bss` as a `static`, so audio data never touches the heap.
pub struct AudioRing<const N: usize> {
    samples: [i16; N],
    generations: [u32; N],
    /// Whether the sample at the same index is the low `i16` half of a raw
    /// `u32` word (`write_u32_words`); always `false` for plain PCM samples.
    /// Eviction uses it to drop a raw word whole rather than splitting it.
    low_half: [bool; N],
    /// Index of the oldest buffered sample.
    head: usize,
    /// Number of valid samples currently buffered (`<= N`).
    len: usize,
    /// Highest generation ever accepted by `write`; used to ignore an
    /// in-flight chunk that arrives after its generation was superseded.
    newest_generation: u32,
    /// The generation whose first overflow has already been reported, so
    /// `first_drop` fires once per recording rather than once per overflow.
    last_drop_generation: Option<u32>,
}

impl<const N: usize> AudioRing<N> {
    pub const fn new() -> Self {
        Self {
            samples: [0; N],
            generations: [0; N],
            low_half: [false; N],
            head: 0,
            len: 0,
            newest_generation: 0,
            last_drop_generation: None,
        }
    }

    /// Empties the buffer and resets the per-generation drop reporting.
    pub fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
        self.last_drop_generation = None;
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

    /// Generation of the oldest buffered sample, or `None` when empty.
    pub fn head_generation(&self) -> Option<u32> {
        if self.len == 0 {
            None
        } else {
            Some(self.generations[self.head])
        }
    }

    /// Appends `samples` tagged with `generation`, discarding the oldest
    /// buffered samples (of any generation) when there isn't room. Never
    /// allocates, never blocks, and never grows past `N`. A write tagged with
    /// a generation older than the newest already accepted is ignored: its
    /// reader has moved on, and buffering it would only orphan samples ahead
    /// of live audio.
    pub fn write(&mut self, generation: u32, samples: &[i16]) -> WriteResult {
        self.write_iter(generation, samples.iter().copied().map(|s| (s, false)))
    }

    /// Appends the little-endian 16-bit halves of each raw I2S FIFO word,
    /// tagged with `generation`, exactly like [`Self::write`]. Each `u32`
    /// becomes two `i16` samples low half first, so a later
    /// little-endian `i16` serialization of the ring reproduces the original
    /// `u32` bytes - this is how the `ptt_raw` diagnostic passthrough streams
    /// unprocessed PIO words through the same allocation-free ring without a
    /// second buffer. Stale-generation handling and overflow reporting match
    /// [`Self::write`].
    pub fn write_u32_words(&mut self, generation: u32, words: &[u32]) -> WriteResult {
        if generation < self.newest_generation {
            return WriteResult {
                dropped: 0,
                first_drop: false,
            };
        }
        self.newest_generation = generation;
        let mut dropped = 0;
        for &word in words {
            // Make room for a whole word before writing either of its halves,
            // so overflow can never leave one half at the head without its
            // partner. Reading such a split word would misalign the `ptt_raw`
            // level meter, which pairs the drained halves back into words.
            self.evict_until_room_for(2, &mut dropped);
            self.push(word as u16 as i16, true, generation);
            self.push((word >> 16) as u16 as i16, false, generation);
        }
        self.finish_write(generation, dropped)
    }

    fn write_iter(
        &mut self,
        generation: u32,
        samples: impl IntoIterator<Item = (i16, bool)>,
    ) -> WriteResult {
        if generation < self.newest_generation {
            return WriteResult {
                dropped: 0,
                first_drop: false,
            };
        }
        self.newest_generation = generation;
        let mut dropped = 0;
        for (sample, low_half) in samples {
            self.evict_until_room_for(1, &mut dropped);
            self.push(sample, low_half, generation);
        }
        self.finish_write(generation, dropped)
    }

    /// Discards the oldest samples until at least `needed` more fit. A raw
    /// word is dropped as a unit (both `i16` halves) whenever the head is one
    /// of its halves, so a word's halves are never split across the head; a
    /// non-word PCM sample is dropped singly.
    fn evict_until_room_for(&mut self, needed: usize, dropped: &mut usize) {
        while self.len > 0 && self.len + needed > N {
            let drop = if self.low_half[self.head] { 2 } else { 1 };
            for _ in 0..drop.min(self.len) {
                self.head = (self.head + 1) % N;
                self.len -= 1;
                *dropped += 1;
            }
        }
    }

    fn push(&mut self, sample: i16, low_half: bool, generation: u32) {
        let tail = (self.head + self.len) % N;
        self.samples[tail] = sample;
        self.generations[tail] = generation;
        self.low_half[tail] = low_half;
        self.len += 1;
    }

    fn finish_write(&mut self, generation: u32, dropped: usize) -> WriteResult {
        let first_drop = dropped > 0 && self.last_drop_generation != Some(generation);
        if dropped > 0 {
            self.last_drop_generation = Some(generation);
        }
        WriteResult {
            dropped,
            first_drop,
        }
    }

    /// Removes up to `out.len()` of the oldest samples, but only while they
    /// belong to `generation`; samples of any other generation are left in
    /// place for their own upload. Samples from *older* generations are
    /// discarded first: their upload has already finished, so no reader will
    /// ever consume them, and leaving them at the head would block the
    /// generation being read. Returns how many samples were written.
    pub fn read(&mut self, generation: u32, out: &mut [i16]) -> usize {
        while self.len > 0 && self.generations[self.head] < generation {
            self.head = (self.head + 1) % N;
            self.len -= 1;
        }
        let mut n = 0;
        while n < out.len() && n < self.len {
            let index = (self.head + n) % N;
            if self.generations[index] != generation {
                break;
            }
            out[n] = self.samples[index];
            n += 1;
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

/// Whether the utterance `generation` has finished capturing.
///
/// `newest_armed` is the highest generation a recording has been armed for;
/// `newest_ended` is the highest generation whose capture has finished.
/// Generations are handed out in order and only one recording runs at a time,
/// so a generation has ended either because a newer one has since been armed,
/// or because that exact generation reported its own end. Keeping this pure
/// (and taking the counters by value) lets host tests drive it directly.
pub fn utterance_ended(newest_armed: u32, newest_ended: u32, generation: u32) -> bool {
    newest_armed > generation || newest_ended >= generation
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
        assert_eq!(ring.head_generation(), None);
    }

    #[test]
    fn read_returns_samples_in_write_order() {
        let mut ring = AudioRing::<8>::new();
        ring.write(1, &[1, 2, 3]);
        let mut out = [0i16; 2];
        assert_eq!(ring.read(1, &mut out), 2);
        assert_eq!(out, [1, 2]);
        assert_eq!(ring.read(1, &mut out), 1);
        assert_eq!(out[0], 3);
        assert_eq!(ring.read(1, &mut out), 0);
        assert!(ring.is_empty());
    }

    #[test]
    fn read_reports_only_available_samples() {
        let mut ring = AudioRing::<8>::new();
        ring.write(1, &[7, 8]);
        let mut out = [0i16; 5];
        assert_eq!(ring.read(1, &mut out), 2);
        assert_eq!(out[..2], [7, 8]);
    }

    #[test]
    fn read_leaves_other_generations_in_place() {
        let mut ring = AudioRing::<8>::new();
        ring.write(1, &[1, 2, 3]);
        ring.write(2, &[4, 5]);
        assert_eq!(ring.head_generation(), Some(1));

        let mut out = [0i16; 8];
        assert_eq!(ring.read(1, &mut out), 3);
        assert_eq!(&out[..3], [1, 2, 3]);
        // Generation 1 is exhausted; asking again must not touch gen 2.
        assert_eq!(ring.read(1, &mut out), 0);
        assert_eq!(ring.head_generation(), Some(2));
        assert_eq!(ring.len(), 2);

        assert_eq!(ring.read(2, &mut out), 2);
        assert_eq!(&out[..2], [4, 5]);
        assert!(ring.is_empty());
    }

    #[test]
    fn stale_write_for_superseded_generation_is_ignored() {
        let mut ring = AudioRing::<16>::new();
        ring.write(2, &[20, 21]);
        let result = ring.write(1, &[10, 11]);
        assert_eq!(result.dropped, 0);
        assert!(!result.first_drop);

        let mut out = [0i16; 8];
        assert_eq!(ring.read(2, &mut out), 2);
        assert_eq!(&out[..2], [20, 21]);
        assert!(ring.is_empty());
    }

    /// Reproduces the ring state a superseded in-flight chunk leaves behind:
    /// an old-generation chunk is written after the next generation is armed
    /// but before that generation has written anything, so the ring cannot
    /// reject it outright. Reading the newer generation must still work.
    #[test]
    fn orphaned_superseded_chunk_does_not_block_newer_reads() {
        let mut ring = AudioRing::<16>::new();
        ring.write(1, &[10, 11]);
        ring.write(1, &[12, 13]);
        ring.write(2, &[20, 21, 22]);

        let mut out = [0i16; 8];
        let n = ring.read(2, &mut out);
        assert_eq!(&out[..n], [20, 21, 22]);
        assert!(ring.is_empty());
    }

    #[test]
    fn overflow_discards_oldest_samples_and_keeps_capacity() {
        let mut ring = AudioRing::<4>::new();
        let result = ring.write(1, &[1, 2, 3, 4, 5, 6]);
        assert_eq!(result.dropped, 2);
        assert_eq!(ring.len(), 4);
        assert_eq!(ring.capacity(), 4);
        let mut out = [0i16; 4];
        assert_eq!(ring.read(1, &mut out), 4);
        assert_eq!(out, [3, 4, 5, 6]);
    }

    #[test]
    fn write_larger_than_capacity_keeps_most_recent_samples() {
        let mut ring = AudioRing::<3>::new();
        let result = ring.write(1, &[1, 2, 3, 4, 5]);
        assert_eq!(result.dropped, 2);
        let mut out = [0i16; 3];
        assert_eq!(ring.read(1, &mut out), 3);
        assert_eq!(out, [3, 4, 5]);
    }

    #[test]
    fn exact_capacity_write_drops_nothing() {
        let mut ring = AudioRing::<4>::new();
        let result = ring.write(1, &[1, 2, 3, 4]);
        assert_eq!(result.dropped, 0);
        assert!(!result.first_drop);
        assert_eq!(ring.len(), 4);
    }

    /// A later generation may overwrite an earlier generation's oldest
    /// samples under pressure, but must never silently discard a whole
    /// earlier utterance's data merely because it started.
    #[test]
    fn new_generation_only_drops_oldest_samples_under_pressure() {
        let mut ring = AudioRing::<8>::new();
        ring.write(1, &[1, 2, 3, 4]);
        let result = ring.write(2, &[5, 6, 7, 8]);
        assert_eq!(result.dropped, 0);
        assert_eq!(ring.len(), 8);

        let mut out = [0i16; 4];
        assert_eq!(ring.read(1, &mut out), 4);
        assert_eq!(out, [1, 2, 3, 4]);
        assert_eq!(ring.read(2, &mut out), 4);
        assert_eq!(out, [5, 6, 7, 8]);
    }

    /// The property the firmware relies on for its single log line: no matter
    /// how much data is pushed at a full ring, `first_drop` reports at most
    /// once per recording, and each new recording gets its own report.
    #[test]
    fn first_drop_fires_exactly_once_per_recording() {
        let mut ring = AudioRing::<8>::new();
        let mut log_count = 0;
        for i in 0..100i16 {
            let result = ring.write(7, &[i, i + 1, i + 2, i + 3, i + 4]);
            if result.first_drop {
                log_count += 1;
            }
        }
        assert_eq!(log_count, 1);
        // A new generation gets its own first-drop report.
        assert!(ring.write(8, &[0; 20]).first_drop);
    }

    /// The `ptt_raw` passthrough reinterprets each captured 32-bit word as
    /// two little-endian `i16` samples, so that re-serializing the ring is
    /// byte-for-byte identical to sending the original `u32` words.
    #[test]
    fn write_u32_words_reproduces_the_word_bytes_little_endian() {
        let mut ring = AudioRing::<8>::new();
        ring.write_u32_words(1, &[0x1234_5678u32, 0xABCD_0001u32]);

        let mut out = [0i16; 8];
        let n = ring.read(1, &mut out);
        assert_eq!(n, 4);
        let mut bytes = [0u8; 8];
        for (i, sample) in out[..n].iter().enumerate() {
            bytes[i * 2..i * 2 + 2].copy_from_slice(&sample.to_le_bytes());
        }
        assert_eq!(bytes, [0x78, 0x56, 0x34, 0x12, 0x01, 0x00, 0xCD, 0xAB]);
    }

    #[test]
    fn write_u32_words_shares_stale_and_overflow_behavior() {
        let mut ring = AudioRing::<2>::new();
        // Two words is four samples against a capacity of two, so the two
        // oldest samples are dropped, exactly as `write` would.
        let result = ring.write_u32_words(1, &[0x0001_0002u32, 0x0003_0004u32]);
        assert_eq!(result.dropped, 2);
        assert!(result.first_drop);
        // A superseded generation is ignored just like `write`.
        let stale = ring.write_u32_words(0, &[0xDEAD_BEEFu32]);
        assert_eq!(
            stale,
            WriteResult {
                dropped: 0,
                first_drop: false
            }
        );
        let mut out = [0i16; 2];
        assert_eq!(ring.read(1, &mut out), 2);
        // Word order is low half then high half, so the two words buffered as
        // [0x0002, 0x0001, 0x0004, 0x0003]; the oldest two are dropped.
        assert_eq!(out[0], 0x0004u16 as i16);
        assert_eq!(out[1], 0x0003u16 as i16);
    }

    /// An odd capacity makes the naive one-`i16`-at-a-time eviction drop an
    /// odd number of halves, which would leave the head on a word's high half.
    /// Raw-word overflow must instead drop whole words, so the drained stream
    /// always pairs back into the original words (what the `ptt_raw` meter
    /// relies on).
    #[test]
    fn word_overflow_never_leaves_a_split_word_at_the_head() {
        let mut ring = AudioRing::<5>::new();
        let words = [
            0x1111_2222u32,
            0x3333_4444u32,
            0x5555_6666u32,
            0x7777_8888u32,
        ];
        for &word in &words {
            ring.write_u32_words(1, &[word]);
        }

        let mut out = [0i16; 8];
        let n = ring.read(1, &mut out);
        assert_eq!(n, 4);
        let mut rebuilt = [0u32; 2];
        for (i, word) in rebuilt.iter_mut().enumerate() {
            *word = (out[2 * i] as u16 as u32) | ((out[2 * i + 1] as u16 as u32) << 16);
        }
        assert_eq!(rebuilt, [words[2], words[3]]);
    }

    #[test]
    fn clear_empties_the_ring() {
        let mut ring = AudioRing::<4>::new();
        ring.write(1, &[1, 2, 3, 4]);
        ring.clear();
        assert!(ring.is_empty());
        let mut out = [0i16; 4];
        assert_eq!(ring.read(1, &mut out), 0);
    }

    /// A slow/unreachable destination can never make the buffer bigger than
    /// its capacity: whatever the write pattern, `len` stays bounded, which
    /// is what keeps memory usage from growing without bound.
    #[test]
    fn capacity_is_a_hard_upper_bound_under_arbitrary_writes() {
        let mut ring = AudioRing::<16>::new();
        let mut next = 0i16;
        let mut burst = [0i16; 100];
        for (generation, len) in [1usize, 3, 7, 16, 64, 5, 100].into_iter().enumerate() {
            for sample in burst.iter_mut().take(len) {
                next = next.wrapping_add(1);
                *sample = next;
            }
            ring.write(generation as u32 + 1, &burst[..len]);
            assert!(ring.len() <= ring.capacity());
        }
        // After 100-sample pressure the buffer is full but still exactly N.
        assert_eq!(ring.len(), 16);
    }

    #[test]
    fn utterance_has_not_ended_while_still_capturing() {
        // The newest generation is still live until it reports its own end;
        // any older generation counts as ended already.
        assert!(!utterance_ended(1, 0, 1));
        assert!(!utterance_ended(3, 1, 3));
        assert!(!utterance_ended(5, 2, 5));
    }

    #[test]
    fn utterance_has_ended_on_its_own_end_or_when_superseded() {
        assert!(utterance_ended(1, 1, 1));
        assert!(utterance_ended(2, 1, 1));
        assert!(utterance_ended(2, 2, 1));
        assert!(!utterance_ended(2, 1, 2));
    }

    /// End-to-end model of the shared-ring lifecycle the firmware runs: a
    /// stalled first utterance (A) is superseded by a second (B) that starts
    /// and finishes before A's connection resolves. A's uploader must stop at
    /// its own end without touching B's audio, and B's uploader must then get
    /// B's untouched audio and its own end.
    #[test]
    fn superseded_utterance_stops_before_newer_audio_and_newer_gets_its_own() {
        const A: u32 = 1;
        const B: u32 = 2;
        let mut ring = AudioRing::<32>::new();

        // A captures, and ends while its upload is still connecting.
        ring.write(A, &[10, 11, 12]);

        // B starts and finishes before A's connect resolves, so at this point
        // A has ended and B is both armed and finished.
        ring.write(B, &[20, 21, 22]);
        let newest_armed = B;
        let newest_ended = B;

        // A's uploader drains only A's audio, then observes that A is over.
        let mut a_out = [0i16; 8];
        let a_len = ring.read(A, &mut a_out);
        assert_eq!(&a_out[..a_len], [10, 11, 12]);
        assert_eq!(ring.read(A, &mut a_out), 0);
        assert!(utterance_ended(newest_armed, newest_ended, A));

        // B's uploader gets B's audio, untouched by A's upload, and its own
        // end signal - so its connection eventually closes too.
        let mut b_out = [0i16; 8];
        let b_len = ring.read(B, &mut b_out);
        assert_eq!(&b_out[..b_len], [20, 21, 22]);
        assert!(utterance_ended(newest_armed, newest_ended, B));
        assert!(ring.is_empty());
    }
}
