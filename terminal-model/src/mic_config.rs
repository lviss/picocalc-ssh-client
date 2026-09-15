//! Hardware-independent resolution of the push-to-talk microphone's runtime
//! settings from persisted config strings.
//!
//! The firmware stores the I2S debug knobs as plain strings in its existing
//! config store (`config set ptt_bits|ptt_rate|ptt_edge|ptt_raw|ptt_gain`, see
//! `src/mic.rs` for the store access). This module holds the pure part - the
//! defaults, the string parsing, the out-of-window fallback, the effective
//! value each key reports, and the console-time validation - so that host
//! tests can exercise the exact decision table the firmware runs, per
//! AGENTS.md's "host-testable logic lives in terminal-model" guidance.
//!
//! The binding contract from the captain's brief:
//!
//! * an unconfigured device behaves exactly as before, i.e. the defaults are
//!   today's proven values (32-bit slots at 16 kHz, default BCLK edge,
//!   extracted PCM rather than raw passthrough);
//! * a value that is absent, malformed, or outside the mic's documented range
//!   is either refused at the console ([`validate_setting`]) or, if it is
//!   already in the store, replaced by the default ([`resolve`]) - never
//!   silently mis-clocked;
//! * a change takes effect on the next recording because the caller re-reads
//!   and re-applies these settings before every capture, assembling the PIO
//!   program at runtime.

use crate::pcm_extract::{MAX_MIC_BCLK_HZ, MIN_MIC_BCLK_HZ, bit_clock_hz, mic_settings_valid};
use alloc::format;
use alloc::string::String;

/// I2S frames a left+right pair per word-select cycle; fixed at two, as the
/// captain settled the channel framing.
pub const CHANNELS: u32 = 2;
/// Default sample rate, in Hz. Matches Whisper's internal resampling target,
/// so there's no benefit to capturing at a higher rate.
pub const DEFAULT_RATE_HZ: u32 = 16_000;
/// Default bits per channel slot. The SPH0645 has a fixed 32-bit slot; the
/// captain's proven-working configuration is 32-bit at 16 kHz (1.024 MHz
/// BCLK, the bottom of the mic's documented window).
pub const DEFAULT_BITS: u32 = 32;
/// Default bit-clock edge polarity: `false` keeps the historical side-set
/// values; `true` inverts the low (bit-clock) bit, shifting sampling by half
/// a BCLK cycle.
pub const DEFAULT_EDGE_FLIP: bool = false;
/// Default capture mode: `false` extracts mono 16-bit PCM; `true` streams the
/// unprocessed PIO FIFO words for offline analysis.
pub const DEFAULT_RAW: bool = false;
/// Default capture-time digital gain: 1, i.e. no scaling, so an unconfigured
/// device behaves exactly as today.
pub const DEFAULT_GAIN: u32 = 1;
/// Lowest accepted `ptt_gain` (no scaling).
pub const MIN_GAIN: u32 = 1;
/// Highest accepted `ptt_gain`. Big enough for the captain's x4096 experiment;
/// the clamp is a safety rail, not a claim that the output is useful there.
pub const MAX_GAIN: u32 = 4096;

/// Config keys for the mic's runtime debug settings, alongside the
/// `ptt_host`/`ptt_port` destination keys.
pub const BITS_KEY: &str = "ptt_bits";
pub const RATE_KEY: &str = "ptt_rate";
pub const EDGE_KEY: &str = "ptt_edge";
pub const RAW_KEY: &str = "ptt_raw";
/// Capture-time digital gain (diagnostic); see [`MicSettings::gain`].
pub const GAIN_KEY: &str = "ptt_gain";

/// Microphone settings resolved from persisted config. Re-read and re-applied
/// at the start of every recording, so a `config set ptt_*` takes effect on
/// the next utterance without a reflash or reboot.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MicSettings {
    /// Bits per channel slot; the PIO loop counter is `bits - 2`.
    pub bits: u32,
    /// I2S sample rate in Hz; the bit clock is `rate * bits * CHANNELS`.
    pub rate: u32,
    /// `true` inverts the BCLK edge the PIO program samples on.
    pub edge_flip: bool,
    /// `true` streams raw FIFO words instead of extracted PCM (the driven
    /// slots are DC-removed and gained first when `gain > 1`).
    pub raw: bool,
    /// Diagnostic capture gain: after each chunk's DC mean is removed, the
    /// driven samples are scaled by this factor with saturating arithmetic, in
    /// both the raw-word and extracted-PCM paths; 1 is a no-op.
    pub gain: u32,
}

impl Default for MicSettings {
    fn default() -> Self {
        Self {
            bits: DEFAULT_BITS,
            rate: DEFAULT_RATE_HZ,
            edge_flip: DEFAULT_EDGE_FLIP,
            raw: DEFAULT_RAW,
            gain: DEFAULT_GAIN,
        }
    }
}

impl MicSettings {
    /// The bit clock these settings produce, in Hz.
    pub fn bclk_hz(&self) -> u32 {
        bit_clock_hz(self.rate, self.bits, CHANNELS)
    }
}

/// Effective settings plus whether an out-of-window stored pair had to be
/// replaced by the default (so the caller can log it once per recording).
pub struct ResolvedSettings {
    pub settings: MicSettings,
    pub fell_back: bool,
}

/// Parse an unsigned integer setting, tolerating surrounding whitespace.
pub fn parse_u32(value: &str) -> Option<u32> {
    value.trim().parse().ok()
}

/// Parse a boolean setting in the forms the console accepts.
pub fn parse_bool(value: &str) -> Option<bool> {
    match value.trim() {
        "1" | "true" | "on" | "yes" => Some(true),
        "0" | "false" | "off" | "no" => Some(false),
        _ => None,
    }
}

/// Whether `key` is one of this module's mic settings.
pub fn owns(key: &str) -> bool {
    matches!(key, BITS_KEY | RATE_KEY | EDGE_KEY | RAW_KEY | GAIN_KEY)
}

/// Resolves the raw stored values (each optional; `None` when the key is
/// absent) against the defaults. A missing or malformed individual value falls
/// back to its own default; if the resulting slot-width/rate pair is outside
/// the mic's documented clock window, the whole pair falls back to the default
/// so the firmware can never silently mis-clock the mic.
pub fn resolve(
    bits: Option<&str>,
    rate: Option<&str>,
    edge: Option<&str>,
    raw: Option<&str>,
    gain: Option<&str>,
) -> ResolvedSettings {
    let mut settings = MicSettings {
        bits: bits.and_then(parse_u32).unwrap_or(DEFAULT_BITS),
        rate: rate.and_then(parse_u32).unwrap_or(DEFAULT_RATE_HZ),
        edge_flip: edge.and_then(parse_bool).unwrap_or(DEFAULT_EDGE_FLIP),
        raw: raw.and_then(parse_bool).unwrap_or(DEFAULT_RAW),
        gain: gain
            .and_then(parse_u32)
            .filter(|g| (MIN_GAIN..=MAX_GAIN).contains(g))
            .unwrap_or(DEFAULT_GAIN),
    };
    let mut fell_back = false;
    if !mic_settings_valid(settings.bits, settings.rate) {
        settings.bits = DEFAULT_BITS;
        settings.rate = DEFAULT_RATE_HZ;
        fell_back = true;
    }
    ResolvedSettings {
        settings,
        fell_back,
    }
}

/// Effective value of a `ptt_*` setting for `config get`, so an unset key
/// reports the value the next recording would actually use. `None` for keys
/// this module does not own.
pub fn effective_setting(key: &str, settings: MicSettings) -> Option<String> {
    match key {
        BITS_KEY => Some(format!("{}", settings.bits)),
        RATE_KEY => Some(format!("{}", settings.rate)),
        EDGE_KEY => Some(format!("{}", settings.edge_flip as u8)),
        RAW_KEY => Some(format!("{}", settings.raw as u8)),
        GAIN_KEY => Some(format!("{}", settings.gain)),
        _ => None,
    }
}

/// Validates a `config set ptt_*` request against the currently effective
/// settings, so an out-of-window or malformed value is refused at the console
/// rather than stored. Keys this module does not own return `Ok(())`.
pub fn validate_setting(current: MicSettings, key: &str, value: &str) -> Result<(), String> {
    if !owns(key) {
        return Ok(());
    }
    let mut candidate = current;
    match key {
        BITS_KEY => {
            candidate.bits = parse_u32(value)
                .ok_or_else(|| String::from("ptt_bits must be a number (bits per channel slot)"))?;
        }
        RATE_KEY => {
            candidate.rate =
                parse_u32(value).ok_or_else(|| String::from("ptt_rate must be a number (Hz)"))?;
        }
        EDGE_KEY => {
            candidate.edge_flip =
                parse_bool(value).ok_or_else(|| String::from("ptt_edge must be 0 or 1"))?;
        }
        RAW_KEY => {
            candidate.raw =
                parse_bool(value).ok_or_else(|| String::from("ptt_raw must be 0 or 1"))?;
        }
        GAIN_KEY => {
            candidate.gain =
                parse_u32(value).ok_or_else(|| String::from("ptt_gain must be a number"))?;
            if !(MIN_GAIN..=MAX_GAIN).contains(&candidate.gain) {
                return Err(format!(
                    "ptt_gain must be between {MIN_GAIN} and {MAX_GAIN}"
                ));
            }
        }
        _ => return Ok(()),
    }
    if !mic_settings_valid(candidate.bits, candidate.rate) {
        return Err(format!(
            "invalid mic clock: ptt_bits={} ptt_rate={} -> BCLK {} Hz, outside the mic's documented {}-{} Hz window",
            candidate.bits,
            candidate.rate,
            candidate.bclk_hz(),
            MIN_MIC_BCLK_HZ,
            MAX_MIC_BCLK_HZ,
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unconfigured_device_uses_todays_proven_defaults() {
        // Nothing stored: every key resolves to the value the captain has been
        // testing with (32-bit slots at 16 kHz, default edge, extracted PCM),
        // and that pair sits exactly at the bottom of the mic's clock window.
        let resolved = resolve(None, None, None, None, None);
        assert_eq!(resolved.settings, MicSettings::default());
        assert!(!resolved.fell_back);
        assert_eq!(resolved.settings.bits, 32);
        assert_eq!(resolved.settings.rate, 16_000);
        assert!(!resolved.settings.edge_flip);
        assert!(!resolved.settings.raw);
        assert_eq!(resolved.settings.bclk_hz(), 1_024_000);
        assert!(mic_settings_valid(DEFAULT_BITS, DEFAULT_RATE_HZ));
        assert_eq!(
            bit_clock_hz(DEFAULT_RATE_HZ, DEFAULT_BITS, CHANNELS),
            1_024_000
        );
    }

    #[test]
    fn a_valid_stored_change_is_used_without_falling_back() {
        // 16-bit slots at 32 kHz is still exactly 1.024 MHz, so it is a legal
        // probe combination and must be taken as-is.
        let resolved = resolve(Some("16"), Some("32000"), Some("1"), Some("1"), None);
        assert!(!resolved.fell_back);
        assert_eq!(resolved.settings.bits, 16);
        assert_eq!(resolved.settings.rate, 32_000);
        assert!(resolved.settings.edge_flip);
        assert!(resolved.settings.raw);
        assert_eq!(resolved.settings.bclk_hz(), 1_024_000);
    }

    #[test]
    fn malformed_individual_values_fall_back_to_their_own_default() {
        // A malformed value must not be taken literally, and must not disturb
        // the other keys.
        let resolved = resolve(
            Some("nonsense"),
            Some("32000"),
            Some("maybe"),
            Some("2"),
            None,
        );
        assert_eq!(resolved.settings.bits, DEFAULT_BITS);
        assert_eq!(resolved.settings.rate, 32_000);
        assert_eq!(resolved.settings.edge_flip, DEFAULT_EDGE_FLIP);
        assert_eq!(resolved.settings.raw, DEFAULT_RAW);
        assert!(!resolved.fell_back);
    }

    #[test]
    fn whitespace_and_the_documented_bool_spellings_are_accepted() {
        let resolved = resolve(Some(" 16 "), Some("32000\n"), Some("on"), Some("off"), None);
        assert_eq!(resolved.settings.bits, 16);
        assert_eq!(resolved.settings.rate, 32_000);
        assert!(resolved.settings.edge_flip);
        assert!(!resolved.settings.raw);
        assert!(!resolved.fell_back);
    }

    #[test]
    fn out_of_window_stored_pair_falls_back_to_default_rather_than_mis_clocking() {
        // The 16-bit-slot-at-16-kHz pair that produced a dead line on real
        // hardware, and an absurd stored rate, must both be rejected as a
        // pair and replaced by the default - never handed to the PIO clock.
        for (bits, rate) in [
            (Some("16"), Some("16000")),
            (Some("4000000000"), Some("16000")),
        ] {
            let resolved = resolve(bits, rate, Some("1"), Some("1"), None);
            assert!(resolved.fell_back, "bits={bits:?} rate={rate:?}");
            assert_eq!(resolved.settings.bits, DEFAULT_BITS);
            assert_eq!(resolved.settings.rate, DEFAULT_RATE_HZ);
            // The non-clock keys are unaffected by the pair fallback.
            assert!(resolved.settings.edge_flip);
            assert!(resolved.settings.raw);
            assert!(mic_settings_valid(
                resolved.settings.bits,
                resolved.settings.rate
            ));
        }
    }

    #[test]
    fn console_accepts_valid_settings_and_reports_them_as_effective() {
        let current = MicSettings::default();
        assert!(validate_setting(current, RATE_KEY, "32000").is_ok());
        assert!(validate_setting(current, BITS_KEY, "16").is_err()); // 16@16k is out of window
        // Applying both changes in a legal order leaves a valid pair.
        let changed = MicSettings {
            bits: 16,
            rate: 32_000,
            edge_flip: true,
            raw: true,
            gain: 256,
        };
        assert!(validate_setting(changed, EDGE_KEY, "1").is_ok());
        assert!(validate_setting(changed, RAW_KEY, "1").is_ok());
        assert_eq!(effective_setting(BITS_KEY, changed).as_deref(), Some("16"));
        assert_eq!(
            effective_setting(RATE_KEY, changed).as_deref(),
            Some("32000")
        );
        assert_eq!(effective_setting(EDGE_KEY, changed).as_deref(), Some("1"));
        assert_eq!(effective_setting(RAW_KEY, changed).as_deref(), Some("1"));
        assert_eq!(effective_setting(GAIN_KEY, changed).as_deref(), Some("256"));
        assert_eq!(effective_setting("ptt_host", changed), None);
    }

    #[test]
    fn console_refuses_malformed_and_out_of_window_values() {
        let current = MicSettings::default();
        for (key, value) in [
            (BITS_KEY, "nonsense"),
            (RATE_KEY, "not-a-rate"),
            (EDGE_KEY, "2"),
            (RAW_KEY, "maybe"),
            (GAIN_KEY, "0"),
            (GAIN_KEY, "4097"),
            (GAIN_KEY, "lots"),
            (BITS_KEY, "16"),         // 512 kHz, below the window
            (RATE_KEY, "128000"),     // 8.192 MHz, above the window
            (BITS_KEY, "7"),          // below the PIO shift-register minimum
            (BITS_KEY, "33"),         // above the PIO shift-register maximum
            (RATE_KEY, "4000000000"), // huge; checked arithmetic must reject, not wrap
        ] {
            assert!(
                validate_setting(current, key, value).is_err(),
                "{key}={value} should be refused"
            );
        }
    }

    #[test]
    fn unowned_keys_are_left_alone_by_validation() {
        // `ptt_host`/`ptt_port` and any other config key must still store
        // through the normal path.
        assert!(validate_setting(MicSettings::default(), "ptt_host", "example.invalid").is_ok());
        assert!(validate_setting(MicSettings::default(), "scroll", "200").is_ok());
        assert!(!owns("ptt_host"));
    }

    #[test]
    fn a_reconfigured_device_reads_back_its_new_settings_on_the_next_recording() {
        // This is the captain's loop: `config set`, then record again. The
        // resolver is the code that runs at capture start, so feeding it the
        // stored strings must reproduce the set values.
        let stored_bits = "24";
        let stored_rate = "21334"; // 24-bit * 21334 * 2 = 1.024032 MHz, just inside the window
        let resolved = resolve(Some(stored_bits), Some(stored_rate), None, None, None);
        assert!(!resolved.fell_back);
        assert_eq!(resolved.settings.bits, 24);
        assert_eq!(resolved.settings.rate, 21_334);
        assert!(mic_settings_valid(
            resolved.settings.bits,
            resolved.settings.rate
        ));
    }

    #[test]
    fn gain_defaults_to_one_and_survives_an_unrelated_clock_fallback() {
        // No key stored: gain is the no-op default.
        assert_eq!(resolve(None, None, None, None, None).settings.gain, 1);
        // A valid gain is taken as stored.
        assert_eq!(
            resolve(None, None, None, None, Some("4096")).settings.gain,
            4096
        );
        // A malformed or out-of-range gain falls back to 1 rather than being
        // handed to the capture path.
        for bad in ["0", "4097", "-1", "lots"] {
            assert_eq!(
                resolve(None, None, None, None, Some(bad)).settings.gain,
                1,
                "ptt_gain={bad}"
            );
        }
        // The out-of-window clock fallback must not disturb gain either.
        let resolved = resolve(Some("16"), Some("16000"), None, None, Some("256"));
        assert!(resolved.fell_back);
        assert_eq!(resolved.settings.gain, 256);
    }
}
