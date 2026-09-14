//! End-to-end host model of the captain's push-to-talk runtime-configuration
//! loop: `config set ptt_*`, then hold F1 and record.
//!
//! This is deliberately not a unit test of any single module. It wires the
//! same public entry points the firmware calls, in the same order, so the
//! captain's actual workflow is exercised as a whole:
//!
//! * `validate_setting` is `src/config.rs`'s `config set` guard;
//! * `resolve` is `mic::load_settings`, which runs at the start of *every*
//!   recording so a change takes effect on the next utterance;
//! * `build_i2s_rx_program` is what `Mic::apply` assembles for the settings;
//! * `extract_left_channel_pcm` / `remove_dc_and_gain_*` are `capture_task`'s
//!   processing of one captured FIFO chunk.
//!
//! The only stand-in is the persisted string store itself: the firmware keeps
//! these keys in `sequential_storage` via `CONFIG`, which needs flash and so
//! cannot run on the host. Every decision around it is the real code.

use pio::{Instruction, InstructionOperands, SetDestination};
use terminal_model::i2s_program::{PROGRAM_SIZE, build_i2s_rx_program};
use terminal_model::mic_config::{
    BITS_KEY, DEFAULT_RATE_HZ, EDGE_KEY, GAIN_KEY, MicSettings, RATE_KEY, RAW_KEY,
    effective_setting, resolve, validate_setting,
};
use terminal_model::pcm_extract::{
    extract_left_channel_pcm, remove_dc_and_gain_samples, remove_dc_and_gain_words,
};

/// The firmware's persisted `config` store, modeled as the string map the mic
/// keys live in. `set`/`get` reproduce the console's observable behavior.
#[derive(Default)]
struct ConfigStore {
    entries: Vec<(String, String)>,
}

impl ConfigStore {
    fn fetch(&self, key: &str) -> Option<String> {
        self.entries
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    }

    fn remove(&mut self, key: &str) {
        self.entries.retain(|(k, _)| k != key);
    }

    /// The settings a recording started right now would use, exactly as
    /// `mic::resolve_settings` computes them from the store.
    fn settings(&self) -> MicSettings {
        resolve(
            self.fetch(BITS_KEY).as_deref(),
            self.fetch(RATE_KEY).as_deref(),
            self.fetch(EDGE_KEY).as_deref(),
            self.fetch(RAW_KEY).as_deref(),
            self.fetch(GAIN_KEY).as_deref(),
        )
        .settings
    }

    /// `config set`: refuse an invalid mic value before it reaches the store,
    /// and otherwise persist it (mirrors `config_command`'s `set` arm, which
    /// validates before storing).
    fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        validate_setting(self.settings(), key, value)?;
        match self.entries.iter_mut().find(|(k, _)| k == key) {
            Some((_, v)) => *v = value.to_string(),
            None => self.entries.push((key.to_string(), value.to_string())),
        }
        Ok(())
    }

    /// `config get`: the value the next recording will actually use.
    fn get(&self, key: &str) -> Option<String> {
        effective_setting(key, self.settings())
    }
}

/// The `set x, N` loop counter of the program `Mic::apply` would load for the
/// settings a recording resolves to.
fn loop_count(program: &pio::Program<PROGRAM_SIZE>) -> u32 {
    match Instruction::decode(program.code[0], program.side_set)
        .expect("assembled instruction decodes")
        .operands
    {
        InstructionOperands::SET {
            destination: SetDestination::X,
            data,
        } => data as u32,
        other => panic!("first instruction was {other:?}, expected `set x, N`"),
    }
}

/// One recording's worth of processing: resolve settings, build the PIO
/// program the SM would run, and turn a captured alternating left/right-slot
/// FIFO chunk into the `i16` stream that goes on the wire.
fn process_chunk(store: &ConfigStore, chunk: &[u32]) -> Vec<i16> {
    let settings = store.settings();
    // `Mic::apply` assembles this before enabling the state machine.
    let program = build_i2s_rx_program(settings.bits, settings.edge_flip);
    assert_eq!(program.code.len(), PROGRAM_SIZE, "program shape is fixed");
    let _ = loop_count(&program);

    if settings.raw {
        let mut words = chunk.to_vec();
        remove_dc_and_gain_words(&mut words, settings.gain);
        words
            .iter()
            .flat_map(|w| [*w as u16 as i16, (*w >> 16) as u16 as i16])
            .collect()
    } else {
        let mut pcm = vec![0i16; chunk.len() / 2];
        extract_left_channel_pcm(chunk, &mut pcm, settings.bits);
        remove_dc_and_gain_samples(&mut pcm, settings.gain);
        pcm
    }
}

/// Rebuilds the original 32-bit FIFO words from the little-endian `i16`
/// halves `ptt_raw` streams, i.e. what a receiver concatenating frames sees.
fn words_from_stream(stream: &[i16]) -> Vec<u32> {
    stream
        .chunks_exact(2)
        .map(|pair| (pair[0] as u16 as u32) | ((pair[1] as u16 as u32) << 16))
        .collect()
}

/// A chunk whose left slots carry `samples` in the top 16 bits (the 32-bit
/// slot's signal field) and whose right (undriven) slots carry a sentinel that
/// must never leak into the left PCM.
fn chunk_with_samples(samples: &[i16]) -> Vec<u32> {
    let mut chunk = Vec::with_capacity(samples.len() * 2);
    for &sample in samples {
        chunk.push((sample as u16 as u32) << 16);
        chunk.push(0x0000_BEEFu32);
    }
    chunk
}

/// The same shape, but with the signal in the slot's low 16 bits, which is
/// where a narrow (16-bit) slot's samples live after extraction.
fn chunk_with_low16_samples(samples: &[i16]) -> Vec<u32> {
    let mut chunk = Vec::with_capacity(samples.len() * 2);
    for &sample in samples {
        chunk.push(sample as u16 as u32);
        chunk.push(0x0000_BEEFu32);
    }
    chunk
}

#[test]
fn unconfigured_device_records_todays_proven_defaults() {
    let store = ConfigStore::default();
    let settings = store.settings();

    // No `config set` at all: every key reports the documented default and the
    // recording resolves to today's proven 32-bit/16 kHz pair.
    assert_eq!(settings, MicSettings::default());
    assert_eq!(store.get(BITS_KEY).as_deref(), Some("32"));
    assert_eq!(store.get(RATE_KEY).as_deref(), Some("16000"));
    assert_eq!(store.get(EDGE_KEY).as_deref(), Some("0"));
    assert_eq!(store.get(RAW_KEY).as_deref(), Some("0"));
    assert_eq!(store.get(GAIN_KEY).as_deref(), Some("1"));
    assert_eq!(settings.bits, 32);
    assert_eq!(settings.rate, DEFAULT_RATE_HZ);
    assert_eq!(settings.bclk_hz(), 1_024_000);

    // 32-bit slot => `set x, bits - 2` = 30, matching the shipped program.
    let program = build_i2s_rx_program(settings.bits, settings.edge_flip);
    assert_eq!(loop_count(&program), 30);

    // Gain 1 is a byte-for-byte no-op: the extracted samples are exactly the
    // top 16 bits of each left slot, with the undriven slots ignored.
    let chunk = chunk_with_samples(&[1000, 1002, 998, 1004]);
    let stream = process_chunk(&store, &chunk);
    assert_eq!(stream, [1000, 1002, 998, 1004]);
}

#[test]
fn captain_config_set_loop_changes_the_very_next_recording() {
    let mut store = ConfigStore::default();
    // The same captured words, processed before and after the reconfiguration.
    let chunk = chunk_with_samples(&[0x1234, 0x5678]);
    let before = process_chunk(&store, &chunk);

    // A slot width that would under-clock the mic at the current rate is
    // refused at the console and leaves the store untouched...
    let refused = store.set(BITS_KEY, "16");
    assert!(refused.is_err(), "16-bit slot at 16 kHz must be refused");
    assert_eq!(store.get(BITS_KEY).as_deref(), Some("32"));
    assert_eq!(store.fetch(BITS_KEY), None, "nothing was persisted");

    // ...but the captain's loop works when the pair stays inside the window:
    // raise the rate first, then narrow the slot, then flip the edge.
    store
        .set(RATE_KEY, "32000")
        .expect("32-bit @ 32 kHz is legal");
    store.set(BITS_KEY, "16").expect("16-bit @ 32 kHz is legal");
    store.set(EDGE_KEY, "1").expect("ptt_edge is 0/1");

    assert_eq!(store.get(BITS_KEY).as_deref(), Some("16"));
    assert_eq!(store.get(RATE_KEY).as_deref(), Some("32000"));
    assert_eq!(store.get(EDGE_KEY).as_deref(), Some("1"));

    let after_settings = store.settings();
    assert_eq!(after_settings.bits, 16);
    assert_eq!(after_settings.rate, 32_000);
    assert!(after_settings.edge_flip);
    // Same documented BCLK, different shape.
    assert_eq!(after_settings.bclk_hz(), 1_024_000);

    // The runtime-assembled program now has a 16-bit loop count...
    let program = build_i2s_rx_program(after_settings.bits, after_settings.edge_flip);
    assert_eq!(loop_count(&program), 14);
    // ...and the edge flip really changes the sampled side-set.
    let normal = build_i2s_rx_program(after_settings.bits, false);
    assert_eq!(
        Instruction::decode(program.code[0], program.side_set)
            .unwrap()
            .side_set
            .unwrap()
            ^ 0b01,
        Instruction::decode(normal.code[0], normal.side_set)
            .unwrap()
            .side_set
            .unwrap()
    );

    // The next recording uses the new slot width: the low 16 bits are now the
    // signal field, so the same captured words extract differently than they
    // did on the unconfigured device.
    let after = process_chunk(&store, &chunk);
    // At 32 bits the extractor took the top 16 bits (`0x1234`/`0x5678`); at
    // 16 bits those sentinel left words have a zero low half, proving the
    // runtime slot width reached the extraction, not just the console.
    assert_eq!(before, [0x1234, 0x5678]);
    assert_eq!(after, [0, 0]);
}

#[test]
fn ptt_gain_is_a_console_knob_over_a_dc_removed_amplifier() {
    let mut store = ConfigStore::default();
    // A quiet signal riding on a large DC plateau, as the SPH0645 produces.
    let samples = [1000i16, 1002, 998, 1004];
    let chunk = chunk_with_samples(&samples);

    // Default gain records the raw (DC-offset) samples unchanged.
    assert_eq!(process_chunk(&store, &chunk), samples);

    // `config set ptt_gain 4`: DC is subtracted first, then the deviation is
    // amplified x4. Mean is 1001, so deviations -1, +1, -3, +3 become -4, 4,
    // -12, 12 - the signal is amplified instead of the offset railing.
    store.set(GAIN_KEY, "4").expect("gain 4 is in range");
    assert_eq!(store.get(GAIN_KEY).as_deref(), Some("4"));
    assert_eq!(process_chunk(&store, &chunk), [-4, 4, -12, 12]);

    // A gain outside the documented 1..=4096 is refused and does not stick.
    assert!(store.set(GAIN_KEY, "0").is_err());
    assert!(store.set(GAIN_KEY, "4097").is_err());
    assert_eq!(store.get(GAIN_KEY).as_deref(), Some("4"));

    // Back to 1: byte-for-byte identical to the un-gained capture again.
    store.set(GAIN_KEY, "1").expect("gain 1 is the no-op");
    assert_eq!(process_chunk(&store, &chunk), samples);

    // Saturation, not wrap: an extreme gain on a full-scale deviation clips
    // at the i16 rails (the mean of the alternating extremes is 0).
    store.set(GAIN_KEY, "4096").expect("gain 4096 is in range");
    let full_scale = chunk_with_samples(&[i16::MIN, i16::MAX, i16::MIN, i16::MAX]);
    assert_eq!(
        process_chunk(&store, &full_scale),
        [i16::MIN, i16::MAX, i16::MIN, i16::MAX]
    );
}

#[test]
fn ptt_raw_gain_removes_dc_before_amplifying_and_leaves_slots_untouched() {
    let mut store = ConfigStore::default();
    // Two right words framing the three left words: only the left words are
    // driven and therefore gain-eligible.
    let chunk = [
        0xFFFF_F000u32,
        0xDEAD_BEEFu32,
        0xFFFF_F800u32,
        0x1234_5678u32,
        0xFFFF_E800u32,
    ];

    // `ptt_raw` with no gain streams the unprocessed words exactly, so a
    // receiver gets the original bytes back.
    store.set(RAW_KEY, "1").expect("ptt_raw is 0/1");
    assert_eq!(words_from_stream(&process_chunk(&store, &chunk)), chunk);

    // With gain, the driven slots have their mean subtracted and are scaled
    // x16; the undriven slots keep their original bytes.
    store.set(GAIN_KEY, "16").expect("gain 16 is in range");
    let gained = words_from_stream(&process_chunk(&store, &chunk));
    assert_eq!(gained[0], 0); // mean-centred
    assert_eq!(gained[1], 0xDEAD_BEEF); // undriven slot untouched
    assert_eq!(gained[2], (2048i32 * 16) as u32);
    assert_eq!(gained[3], 0x1234_5678); // undriven slot untouched
    assert_eq!(gained[4], (-2048i32 * 16) as u32);
}

#[test]
fn config_remove_restores_the_default_for_the_next_recording() {
    let mut store = ConfigStore::default();
    store.set(RATE_KEY, "32000").expect("legal");
    store.set(BITS_KEY, "16").expect("legal");
    assert_eq!(store.settings().bits, 16);
    assert_eq!(store.settings().rate, 32_000);
    assert_eq!(store.get(RATE_KEY).as_deref(), Some("32000"));

    // `config rm ptt_rate`: the malformed/incomplete stored pair is now
    // out-of-window (16-bit @ 16 kHz), which must resolve back to the default
    // rather than mis-clock the mic.
    store.remove(RATE_KEY);
    assert_eq!(store.get(RATE_KEY).as_deref(), Some("16000"));
    assert_eq!(store.settings(), MicSettings::default());

    // `config rm` of every mic key returns the device to the unconfigured
    // defaults, and the next recording is byte-identical to the fresh device.
    store.remove(BITS_KEY);
    store.remove(GAIN_KEY);
    store.remove(EDGE_KEY);
    store.remove(RAW_KEY);
    assert_eq!(store.settings(), MicSettings::default());
    assert_eq!(
        process_chunk(&store, &chunk_with_samples(&[7, -7])),
        [7, -7]
    );
}

/// Prints a transcript of the captain's real workflow - `config set`, then
/// hold F1 - using the same decision functions the console and capture path
/// call. Assertions pin the transcript's substance so this is a real test,
/// not just logging; run with `--nocapture` (see the evidence artifact) to
/// read the session.
#[test]
fn console_session_transcript_config_set_then_record() {
    let mut store = ConfigStore::default();
    let mut transcript = String::new();

    fn show_get(store: &ConfigStore, key: &str, transcript: &mut String) {
        transcript.push_str(&format!(
            "$ config get {key}\n{}\n",
            store.get(key).unwrap_or_default()
        ));
    }
    show_get(&store, BITS_KEY, &mut transcript);
    show_get(&store, RATE_KEY, &mut transcript);
    show_get(&store, EDGE_KEY, &mut transcript);
    show_get(&store, RAW_KEY, &mut transcript);
    show_get(&store, GAIN_KEY, &mut transcript);

    // A value that would under-clock the mic is refused with the real message
    // the console prints, and is not stored.
    match store.set(BITS_KEY, "16") {
        Err(message) => transcript.push_str(&format!("$ config set ptt_bits 16\n{message}\n")),
        Ok(()) => panic!("16-bit at 16 kHz must be refused"),
    }
    assert_eq!(store.fetch(BITS_KEY), None);

    // The captain's ordered probe: raise the rate, then narrow the slot, flip
    // the edge, and raise the diagnostic gain.
    for (key, value) in [
        (RATE_KEY, "32000"),
        (BITS_KEY, "16"),
        (EDGE_KEY, "1"),
        (GAIN_KEY, "256"),
    ] {
        match store.set(key, value) {
            Ok(()) => transcript.push_str(&format!("$ config set {key} {value}\nOK\n")),
            Err(message) => panic!("{key}={value} should be accepted: {message}"),
        }
    }
    for key in [BITS_KEY, RATE_KEY, EDGE_KEY, GAIN_KEY] {
        show_get(&store, key, &mut transcript);
    }

    // Now "hold F1": the next recording resolves the stored settings, builds
    // the runtime PIO program, and processes one captured chunk.
    let settings = store.settings();
    let program = build_i2s_rx_program(settings.bits, settings.edge_flip);
    let chunk = chunk_with_low16_samples(&[1000, 1002, 998, 1004]);
    let stream = process_chunk(&store, &chunk);

    transcript.push_str(&format!(
        "--- hold F1: next recording ---\n\
         resolved: bits={} rate={} edge={} raw={} gain={} bclk={}\n\
         PIO `set x` loop count: {}\n\
         captured left-slot samples (quiet, DC-offset): {:?}\n\
         streamed PCM: {:?}\n",
        settings.bits,
        settings.rate,
        settings.edge_flip as u8,
        settings.raw as u8,
        settings.gain,
        settings.bclk_hz(),
        loop_count(&program),
        [1000, 1002, 998, 1004],
        stream,
    ));

    assert_eq!(settings.bits, 16);
    assert_eq!(settings.rate, 32_000);
    assert_eq!(settings.gain, 256);
    assert_eq!(loop_count(&program), 14);
    assert_eq!(stream, [-256, 256, -768, 768]);

    print!("{transcript}");
}
