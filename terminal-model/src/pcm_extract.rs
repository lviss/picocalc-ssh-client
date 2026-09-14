//! Hardware-independent PCM extraction arithmetic for the I2S mic capture.
//!
//! The mic (an Adafruit SPH0645) has a fixed 32-bit-per-channel I2S slot
//! (64fs total per L+R frame - see `mic.rs`'s `BIT_DEPTH` for the full
//! derivation against its documented clock table). The PIO program's RX FIFO
//! therefore delivers one 32-bit word per channel slot, alternating left and
//! right, rather than one word per combined L+R frame. This module holds the
//! two pieces of that arithmetic that don't need any hardware to verify:
//! the bit-clock frequency the firmware must generate, and how a mono PCM
//! stream is recovered from the alternating raw words. Keeping this here
//! (free of embassy/`crate` dependencies) lets host tests exercise the exact
//! arithmetic the firmware runs, per AGENTS.md's "host-testable logic lives
//! in terminal-model" guidance.

/// The bit-clock (BCLK) frequency, in Hz, this mic requires for a given
/// sample rate: `sample_rate_hz * bits_per_channel_slot * channels`. For the
/// SPH0645's fixed 32-bit slot at 16 kHz this is 1.024 MHz, matching its
/// documented supported-clock table (1.024 MHz-4.096 MHz for 16 kHz-64 kHz).
pub const fn bit_clock_hz(sample_rate_hz: u32, bits_per_channel_slot: u32, channels: u32) -> u32 {
    sample_rate_hz * bits_per_channel_slot * channels
}

/// Recovers mono 16-bit PCM samples from raw I2S RX FIFO words, where each
/// word is one channel's full slot (MSB-first, alternating left then right)
/// rather than a combined L+R pair. Keeps only the even-indexed (left-slot)
/// words - swap to odd-indexed if a mic is instead wired to the right slot -
/// and takes each kept word's top 16 bits as the PCM sample. `pcm` is filled
/// from `raw.len() / 2` words; any extra `pcm` entries beyond that are left
/// untouched.
pub fn extract_left_channel_pcm(raw: &[u32], pcm: &mut [i16]) {
    for (dst, word) in pcm.iter_mut().zip(raw.iter().step_by(2)) {
        *dst = (*word >> 16) as i16;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sph0645_bit_clock_matches_documented_table_at_16khz() {
        // 16 kHz * 32-bit slot * 2 channels = 1.024 MHz, exactly the clock
        // this mic's own datasheet-derived table requires at this sample
        // rate - see mic.rs's BIT_DEPTH comment for the source. This guards
        // against an accidental regression back to a 16-bit slot (512 kHz,
        // exactly half), which is what originally shipped and produced
        // garbage on real hardware.
        assert_eq!(bit_clock_hz(16_000, 32, 2), 1_024_000);
    }

    #[test]
    fn extract_left_channel_pcm_keeps_only_even_indexed_words() {
        // Alternating left/right words: left words carry a distinct pattern
        // in their top 16 bits so we can tell them apart from right words if
        // the extraction ever accidentally picks the wrong parity.
        let raw = [
            0x1111_0000u32, // left  (index 0)
            0x2222_0000u32, // right (index 1) - must be skipped
            0x3333_0000u32, // left  (index 2)
            0x4444_0000u32, // right (index 3) - must be skipped
        ];
        let mut pcm = [0i16; 2];
        extract_left_channel_pcm(&raw, &mut pcm);
        assert_eq!(pcm, [0x1111u16 as i16, 0x3333u16 as i16]);
    }

    #[test]
    fn extract_left_channel_pcm_takes_top_16_of_each_32_bit_word() {
        // MSB-first (ShiftDirection::Left): the top 16 bits of a captured
        // word are this mic's most significant 16 (of its 18 significant)
        // bits. A word with a nonzero low half must not leak into the
        // extracted sample.
        let raw = [0xABCD_FFFFu32];
        let mut pcm = [0i16; 1];
        extract_left_channel_pcm(&raw, &mut pcm);
        assert_eq!(pcm[0], 0xABCDu16 as i16);
    }

    #[test]
    fn extract_left_channel_pcm_stops_at_the_shorter_of_the_two_buffers() {
        // Fewer raw words than pcm slots: only as many samples as available
        // word-pairs are written, matching Iterator::zip's short-circuit -
        // this is what lets capture_task always pass a full-size raw buffer
        // without needing to track a partial-chunk count itself.
        let raw = [0x1111_0000u32, 0x2222_0000u32];
        let mut pcm = [0x7fffi16; 3];
        extract_left_channel_pcm(&raw, &mut pcm);
        assert_eq!(pcm, [0x1111u16 as i16, 0x7fff, 0x7fff]);
    }
}
