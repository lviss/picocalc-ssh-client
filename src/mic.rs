//! Push-to-talk voice capture: a PIO-driven I2S RX driver for a digital mic
//! wired to the expansion-header pins freed by dropping the slow PSRAM path
//! (see AGENTS.md and `psram.rs`), plus the task that streams captured audio
//! to a configurable network host.
//!
//! Audio is staged between the capture and upload tasks in a fixed,
//! allocation-free static ring buffer (`terminal_model::audio_ring`), not on
//! the heap. Capture never blocks and never allocates; when the network side
//! falls behind, the ring drops its oldest samples, so a slow or unreachable
//! `ptt_host` can at worst lose audio, never stall the I2S clock or exhaust
//! the firmware heap. This holds regardless of whether a PSRAM heap tier is
//! present or working.
//!
//! The I2S slot width, sample rate, bit-clock edge polarity, raw-passthrough
//! mode and diagnostic capture gain are runtime settings read from the
//! persisted config store (`ptt_bits`/`ptt_rate`/`ptt_edge`/`ptt_raw`/
//! `ptt_gain`, see [`load_settings`]) at the start of every recording. The PIO
//! program is assembled on the device at that point rather than by `pio_asm!`,
//! so a `config set` takes effect on the next utterance without a reflash or
//! reboot. An out-of-window slot/rate pair is refused at the console and falls
//! back to the default rather than silently mis-clocking the mic.
//!
//! Wire format: see AGENTS.md's push-to-talk entry for the authoritative
//! framing contract a receiving process must speak (one TCP connection per
//! utterance, opened on button press and closed on release; a 4-byte
//! little-endian length prefix per frame, then mono PCM at `ptt_rate` - or the
//! raw FIFO words under `ptt_raw=1`).

use crate::Irqs;
use crate::config::{CONFIG, StrValue};
use crate::net::stack;
use crate::screen::SCREEN;
use alloc::string::String;
use core::sync::atomic::{AtomicU32, Ordering};
use embassy_executor::Spawner;
use embassy_futures::select::select;
use embassy_net::IpEndpoint;
use embassy_net::dns::{DnsQueryType, DnsSocket};
use embassy_net::tcp::TcpSocket;
use embassy_rp::PeripheralRef;
use embassy_rp::clocks::clk_sys_freq;
use embassy_rp::peripherals::{DMA_CH4, PIN_2, PIN_3, PIN_21, PIO2};
use embassy_rp::pio::{
    Config, Direction, FifoJoin, LoadedProgram, Pin, Pio, ShiftConfig, ShiftDirection,
};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, with_timeout};
use embedded_io_async::Write as _;
use fixed::traits::ToFixed;
use terminal_model::audio_ring::{AudioRing, utterance_ended};
use terminal_model::i2s_program::build_i2s_rx_program;
use terminal_model::mic_config::{
    BITS_KEY, CHANNELS, DEFAULT_RATE_HZ, EDGE_KEY, GAIN_KEY, MicSettings, RATE_KEY, RAW_KEY,
    effective_setting as mic_effective_setting, reconcile as reconcile_mic_settings,
    validate_setting as validate_mic_setting,
};
use terminal_model::pcm_extract::{
    ac_rms_level, ac_rms_level_words, bit_clock_hz, extract_left_channel_pcm,
    remove_dc_and_gain_samples, remove_dc_and_gain_words,
};

extern crate alloc;

/// 25 ms per frame at the default rate, comfortably inside the brief's
/// 20-50 ms guidance. A chunk is a fixed number of samples, so a non-default
/// `ptt_rate` changes its duration but not the buffer sizes.
const SAMPLES_PER_CHUNK: usize = DEFAULT_RATE_HZ as usize / 40;
/// Capacity of the shared capture/upload ring, in `i16` samples: 2048 samples
/// at 16 kHz is 128 ms of audio, enough to absorb ordinary connection-setup
/// and scheduling jitter without being a meaningful memory cost. This buffer
/// is a plain `static` compiled into `.bss`, so - unlike the heap-allocated
/// per-chunk `Box`es it replaces - it does not draw on the 64 KiB `DualHeap`
/// the WiFi/TCP/SSH stack and screen scrollback share, and therefore cannot
/// exhaust that heap when the network side falls behind (it drops its oldest
/// samples instead). The behavior does not depend on a PSRAM heap tier
/// existing or working.
const RING_SAMPLES: usize = 2048;
/// Upper bound on DNS + TCP connect for one utterance, so an unreachable
/// `ptt_host` cannot leave that utterance's upload pending indefinitely
/// (smoltcp's SYN retry backoff can otherwise run for a long time).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Upper bound on a single push-to-talk recording. A `Released` report is the
/// normal way to end one, but the keyboard co-processor link can drop a
/// transition (missed poll, I2C glitch) with no automatic recovery until some
/// other event arrives; this caps worst-case exposure (mic powered, I2S clock
/// running, "recording..." overlay stuck) to a minute, far longer than any
/// normal utterance.
const MAX_RECORDING_DURATION: Duration = Duration::from_secs(60);

/// Reads the `ptt_*` mic keys and resolves them against their defaults via
/// [`terminal_model::mic_config::reconcile`]: a missing or malformed individual
/// value falls back to its own default, and an out-of-window slot/rate pair
/// falls back to the default rather than silently mis-clocking the mic. A
/// stored key that no longer matches its effective value is rewritten, so the
/// store, `config get`/`list`, and the next recording all agree.
async fn resolve_settings() -> terminal_model::mic_config::ResolvedSettings {
    let mut config = CONFIG.get().lock().await;
    let bits = config.fetch(BITS_KEY).await.ok().flatten();
    let rate = config.fetch(RATE_KEY).await.ok().flatten();
    let edge = config.fetch(EDGE_KEY).await.ok().flatten();
    let raw = config.fetch(RAW_KEY).await.ok().flatten();
    let gain = config.fetch(GAIN_KEY).await.ok().flatten();
    let (resolved, fixes) = reconcile_mic_settings(
        bits.as_ref().map(|v| v.as_str()),
        rate.as_ref().map(|v| v.as_str()),
        edge.as_ref().map(|v| v.as_str()),
        raw.as_ref().map(|v| v.as_str()),
        gain.as_ref().map(|v| v.as_str()),
    );
    for fix in &fixes {
        if let Ok(value) = TryInto::<StrValue>::try_into(fix.value.as_str()) {
            let _ = config.store(fix.key, value).await;
        }
    }
    resolved
}

/// Settings for the recording about to start, logging if a stored value had to
/// be replaced by the default.
pub async fn load_settings() -> MicSettings {
    let resolved = resolve_settings().await;
    if resolved.fell_back {
        print!("ptt: stored mic clock settings out of range, using defaults\r\n");
    }
    resolved.settings
}

/// Validates a `config set ptt_*` request against the currently effective
/// settings, so an out-of-window or malformed value is refused at the console
/// rather than stored. Keys this module does not own return `Ok(())`.
pub async fn validate_config_setting(key: &str, value: &str) -> Result<(), String> {
    let current = resolve_settings().await.settings;
    validate_mic_setting(current, key, value)
}

/// Effective value of a `ptt_*` setting for `config get`; `None` for keys this
/// module does not own. Shows the default when the key is unset.
pub async fn effective_setting(key: &str) -> Option<String> {
    let settings = resolve_settings().await.settings;
    mic_effective_setting(key, settings)
}

/// Samples captured by `capture_task`, drained by `ptt_upload_task`. A fixed
/// `static` (never heap-allocated, never resized), guarded by an
/// `embassy_sync` mutex rather than held lock-free per `heap.rs`'s CAS
/// caveat. Because `AudioRing::write` never blocks, `capture_task` can deposit
/// samples cooperatively regardless of whether the upload task has connected
/// yet, so there is no connect-vs-capture race to manage. Each sample carries
/// the utterance generation that produced it, so overlapping utterances can
/// share the buffer without one's audio (or end) leaking into the other's
/// connection.
static PCM_RING: Mutex<CriticalSectionRawMutex, AudioRing<RING_SAMPLES>> =
    Mutex::new(AudioRing::new());

/// Highest generation a recording has been armed for; bumped by
/// `start_recording` and read by both tasks. Generations are what distinguish
/// overlapping utterances, so a later recording can never be mistaken for (or
/// consume the end of) an earlier one still being uploaded.
static CURRENT_GEN: AtomicU32 = AtomicU32::new(0);
/// Highest generation whose capture has finished. Monotonic, because only one
/// recording runs at a time.
static ENDED_GEN: AtomicU32 = AtomicU32::new(0);

/// Active recording generation, or 0 when idle. Storing the generation rather
/// than a bool lets the duration cap stop only its own recording.
static RECORDING: AtomicU32 = AtomicU32::new(0);
/// Generation whose capture first overflowed the ring and has not yet been
/// reported, or 0. `capture_task` records this instead of printing, because
/// `print!` awaits the screen lock and would stall the I2S DMA pull.
static OVERFLOW_NOTICE_GEN: AtomicU32 = AtomicU32::new(0);
/// Generation that hit `MAX_RECORDING_DURATION` and has not yet been reported,
/// set for the same reason as `OVERFLOW_NOTICE_GEN`.
static CAP_NOTICE_GEN: AtomicU32 = AtomicU32::new(0);

/// Latest chunk's AC RMS level (in `i16` counts), for the on-screen level
/// meter. Updated by `capture_task` once per chunk with integer-only work, and
/// reset to 0 when a recording ends. The screen painter polls it, so the
/// capture path never takes the screen lock.
static PTT_LEVEL: AtomicU32 = AtomicU32::new(0);

/// Current push-to-talk input level for the on-screen meter; 0 when idle.
/// Deliberately the DC-removed (AC) level, so a mic sitting on its noise floor
/// reads near zero instead of being pinned by its DC offset.
pub fn level() -> u32 {
    PTT_LEVEL.load(Ordering::Acquire)
}

static START_SIGNAL: Signal<CriticalSectionRawMutex, ()> = Signal::new();
/// Wakes `ptt_upload_task` when `capture_task` has deposited samples. A
/// `Signal` coalesces, which is exactly the needed contract: one pending wake
/// means "there may be samples to drain", and the drain loop empties
/// everything available before waiting again. Correctness comes from
/// `CURRENT_GEN`/`ENDED_GEN`, not from this signal, so coalescing across
/// utterances is harmless.
static DATA_READY: Signal<CriticalSectionRawMutex, ()> = Signal::new();
/// Wake-up hint for the same reason as `DATA_READY`; the actual end of an
/// utterance is decided by [`utterance_ended`] against the generation
/// counters.
static STREAM_ENDED: Signal<CriticalSectionRawMutex, ()> = Signal::new();
/// Separate from `START_SIGNAL` (each `Signal` has exactly one waiter:
/// `capture_task` waits on `START_SIGNAL`, `ptt_upload_task` on this one) so
/// the upload task can begin DNS/TCP connect as soon as a recording starts,
/// while `capture_task` fills the static ring.
static UPLOAD_START_SIGNAL: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Begins push-to-talk capture; a no-op if already recording. Called from
/// `keyboard.rs` on `(KeyState::Pressed, Key::F1)` (with no modifiers held).
pub async fn start_recording() {
    if RECORDING.load(Ordering::Acquire) == 0 {
        // A new generation identifies this utterance for the rest of its
        // life. The ring is deliberately *not* cleared here: a previous
        // utterance's undrained audio must stay available to the connection
        // that owns it, and the generation tags keep the two separate.
        let generation = CURRENT_GEN.fetch_add(1, Ordering::AcqRel) + 1;
        RECORDING.store(generation, Ordering::Release);
        SCREEN
            .get()
            .lock()
            .await
            .show_overlay(String::from("recording..."));
        START_SIGNAL.signal(());
        UPLOAD_START_SIGNAL.signal(());
    }
}

/// Whether a push-to-talk recording is currently active. `keyboard.rs` checks
/// this on `(KeyState::Released, Key::F1)` so the matching release always
/// stops capture regardless of which modifiers are held at that instant.
pub fn is_recording() -> bool {
    RECORDING.load(Ordering::Acquire) != 0
}

/// Ends push-to-talk capture; a no-op if not recording. Called from
/// `keyboard.rs` on `(KeyState::Released, Key::F1)` while `is_recording()`.
pub async fn stop_recording() {
    if RECORDING.swap(0, Ordering::AcqRel) != 0 {
        SCREEN.get().lock().await.clear_overlay();
    }
}

/// PIO-backed I2S RX (microphone) driver. This is the mirror image of
/// embassy-rp's `pio_programs::i2s::PioI2sOut` (`in pins, 1` capturing
/// instead of `out pins, 1` emitting, on the same word/bit-clock side-set
/// shape) — embassy-rp ships no I2S RX driver, so this is written directly
/// against the public `embassy_rp::pio` API rather than vendored, following
/// this project's existing `psram.rs` precedent for a custom PIO program.
///
/// It owns the whole `Pio` block plus its pins so [`Self::apply`] can rebuild
/// the runtime-assembled program and swap the loaded one as settings change.
struct Mic {
    pio: Pio<'static, PIO2>,
    dma_ch: PeripheralRef<'static, DMA_CH4>,
    bclk: Pin<'static, PIO2>,
    ws: Pin<'static, PIO2>,
    sd: Pin<'static, PIO2>,
    /// Settings currently loaded into the SM, or `None` before the first
    /// recording; lets `apply` skip redundant reloads.
    applied: Option<MicSettings>,
    /// Instruction memory of the currently loaded program, freed before a new
    /// one is loaded so repeated reconfiguration cannot exhaust PIO RAM.
    loaded: Option<LoadedProgram<'static, PIO2>>,
}

impl Mic {
    fn set_enabled(&mut self, enabled: bool) {
        self.pio.sm0.set_enable(enabled);
    }

    async fn capture(&mut self, buf: &mut [u32]) {
        self.pio
            .sm0
            .rx()
            .dma_pull(self.dma_ch.reborrow(), buf, false)
            .await;
    }

    /// (Re)configures the PIO program and clock for `settings`, freeing the
    /// previously loaded program. The program is rebuilt and reloaded only
    /// when `settings` changed; the state-machine reset and clock/pin config
    /// run on every call.
    fn apply(&mut self, settings: MicSettings) {
        self.pio.sm0.set_enable(false);
        self.pio.sm0.restart();
        self.pio.sm0.clear_fifos();

        if self.applied != Some(settings) {
            let program = build_i2s_rx_program(settings.bits, settings.edge_flip);
            if let Some(old) = self.loaded.take() {
                // SAFETY: the state machine was disabled and restarted above,
                // so it is not executing the instruction memory being freed.
                unsafe { self.pio.common.free_instr(old.used_memory) };
            }
            self.loaded = Some(self.pio.common.load_program(&program));
        }

        let loaded = self
            .loaded
            .take()
            .expect("PIO program must be loaded before applying its config");
        let mut cfg = Config::default();
        cfg.use_program(&loaded, &[&self.bclk, &self.ws]);
        cfg.set_in_pins(&[&self.sd]);
        let clock_frequency = bit_clock_hz(settings.rate, settings.bits, CHANNELS);
        cfg.clock_divider = (clk_sys_freq() as f64 / clock_frequency as f64 / 2.).to_fixed();
        // One autopush per channel slot regardless of width, so FIFO words
        // are always left, right, left, ... and can be sliced by parity.
        cfg.shift_in = ShiftConfig {
            threshold: settings.bits as u8,
            direction: ShiftDirection::Left,
            auto_fill: true,
        };
        // Doubles RX FIFO depth since TX is unused; mirrors PioI2sOut.
        cfg.fifo_join = FifoJoin::RxOnly;

        self.pio.sm0.set_config(&cfg);
        self.pio.sm0.set_pin_dirs(Direction::In, &[&self.sd]);
        self.pio
            .sm0
            .set_pin_dirs(Direction::Out, &[&self.bclk, &self.ws]);
        self.loaded = Some(loaded);
        self.applied = Some(settings);
    }
}

/// Claims PIO2 (unclaimed elsewhere in this codebase; PIO0 is WiFi, PIO1 is
/// now unused since the slow PSRAM path was dropped) and the expansion-header
/// pins that path used to claim, and spawns the capture and network-upload
/// tasks. `bclk`/`ws`/`sd` must be `PIN_2`/`PIN_3`/`PIN_21` per AGENTS.md's
/// pin contract.
pub fn init_mic(
    spawner: &Spawner,
    pio2: PIO2,
    bclk: PIN_2,
    ws: PIN_3,
    sd: PIN_21,
    dma_ch4: DMA_CH4,
) {
    let mut pio = Pio::new(pio2, Irqs);
    let bclk = pio.common.make_pio_pin(bclk);
    let ws = pio.common.make_pio_pin(ws);
    let sd = pio.common.make_pio_pin(sd);

    // Left unconfigured and disabled (no clocks driven, no mic power/noise)
    // until the first recording, when `capture_task` resolves the runtime
    // settings and `Mic::apply` builds the program for them.
    let mic = Mic {
        pio,
        dma_ch: PeripheralRef::new(dma_ch4),
        bclk,
        ws,
        sd,
        applied: None,
        loaded: None,
    };

    spawner.must_spawn(capture_task(mic));
    spawner.must_spawn(ptt_upload_task());
}

#[embassy_executor::task]
async fn capture_task(mut mic: Mic) {
    loop {
        START_SIGNAL.wait().await;
        let generation = CURRENT_GEN.load(Ordering::Acquire);
        // Resolve and apply the runtime debug configuration before the I2S
        // clock starts, so a `config set ptt_*` affects this very utterance
        // (no rebuild, reflash or reboot).
        let settings = load_settings().await;
        mic.apply(settings);
        mic.set_enabled(true);
        let started = Instant::now();
        // Ends when the button is released, when a newer recording supersedes
        // this one, or on the recording-duration cap below.
        while RECORDING.load(Ordering::Acquire) == generation
            && CURRENT_GEN.load(Ordering::Acquire) == generation
        {
            // One FIFO word is one channel slot (see `MicSettings::bits`), not
            // a combined L+R pair: the PIO program pushes left, then right,
            // alternating, so a chunk's worth of *mono* samples needs twice as
            // many raw words.
            let mut raw = [0u32; SAMPLES_PER_CHUNK * 2];
            mic.capture(&mut raw).await;
            if CURRENT_GEN.load(Ordering::Acquire) != generation {
                continue;
            }

            // Never blocks and never allocates: if the network side is
            // behind, the ring drops its oldest samples instead of stalling
            // the DMA pull that keeps the I2S clocks running.
            let result = if settings.raw {
                // `ptt_raw`: bypass extraction and hand the ring the raw FIFO
                // words as little-endian i16 halves, so re-serializing them
                // reproduces the exact unprocessed I2S word stream. The gain
                // knob removes the driven slot's DC offset before amplifying,
                // so it reveals signal rather than railing on the offset.
                PTT_LEVEL.store(ac_rms_level_words(&raw, settings.bits), Ordering::Release);
                remove_dc_and_gain_words(&mut raw, settings.bits, settings.gain);
                PCM_RING.lock().await.write_u32_words(generation, &raw)
            } else {
                // Keep only the even-indexed (left-slot) words and drop the
                // odd-indexed (right-slot) ones the mic never drives, matching
                // this driver's left-slot pin/wiring contract in AGENTS.md
                // (swap to odd-indexed words if a captain instead wires the mic
                // to the right slot). ShiftDirection::Left means MSB-first, and
                // `extract_left_channel_pcm` takes the top 16 bits of the
                // configured slot width.
                let mut pcm = [0i16; SAMPLES_PER_CHUNK];
                extract_left_channel_pcm(&raw, &mut pcm, settings.bits);
                // Publish the AC level for the overlay meter before applying
                // gain, so the meter shows the mic's real input, not the
                // diagnostic gain.
                PTT_LEVEL.store(ac_rms_level(&pcm), Ordering::Release);
                // `ptt_gain`: the captain's capture-time gain experiment,
                // applied after DC removal so a useful gain reveals the AC
                // signal instead of immediately railing on the offset.
                remove_dc_and_gain_samples(&mut pcm, settings.gain);
                PCM_RING.lock().await.write(generation, &pcm)
            };
            if result.first_drop {
                OVERFLOW_NOTICE_GEN.store(generation, Ordering::Release);
            }
            DATA_READY.signal(());

            if started.elapsed() >= MAX_RECORDING_DURATION {
                CAP_NOTICE_GEN.store(generation, Ordering::Release);
                let _ =
                    RECORDING.compare_exchange(generation, 0, Ordering::AcqRel, Ordering::Acquire);
                break;
            }
        }
        mic.set_enabled(false);
        PTT_LEVEL.store(0, Ordering::Release);
        ENDED_GEN.fetch_max(generation, Ordering::AcqRel);
        STREAM_ENDED.signal(());
    }
}

/// Sends one wire frame: a 4-byte little-endian byte count, then the samples.
/// Uses only fixed stack buffers (no heap) so the upload hot path cannot
/// allocate.
async fn send_chunk(socket: &mut TcpSocket<'_>, chunk: &[i16]) -> bool {
    const BATCH_SAMPLES: usize = 64;
    let byte_len = (chunk.len() * 2) as u32;
    if socket.write_all(&byte_len.to_le_bytes()).await.is_err() {
        return false;
    }
    let mut bytes = [0u8; BATCH_SAMPLES * 2];
    for batch in chunk.chunks(BATCH_SAMPLES) {
        for (i, sample) in batch.iter().enumerate() {
            bytes[i * 2..i * 2 + 2].copy_from_slice(&sample.to_le_bytes());
        }
        if socket.write_all(&bytes[..batch.len() * 2]).await.is_err() {
            return false;
        }
    }
    true
}

/// Emits any one-shot diagnostics that `capture_task` recorded and dismisses
/// the overlay a capped recording left up. Runs on the upload task, never on
/// the DMA capture path, so the screen lock can be held by a repaint without
/// stalling the I2S clocks.
async fn emit_pending_notices() {
    if OVERFLOW_NOTICE_GEN.swap(0, Ordering::AcqRel) != 0 {
        print!("ptt: upload can't keep up, dropping oldest audio\r\n");
    }
    let capped_generation = CAP_NOTICE_GEN.swap(0, Ordering::AcqRel);
    if capped_generation != 0 {
        print!("ptt: recording exceeded 60s cap, stopping\r\n");
        let mut screen = SCREEN.get().lock().await;
        if CURRENT_GEN.load(Ordering::Acquire) == capped_generation {
            screen.clear_overlay();
        }
    }
}

/// Sends one utterance's audio over its own connection: connect (bounded by
/// `CONNECT_TIMEOUT`), drain every sample tagged `generation` until that
/// generation ends, then close the connection. Samples belonging to any other
/// generation are left in the ring for their own upload, and the end
/// condition is [`utterance_ended`] on the generation counters - never a
/// shared signal - so a later recording can neither have its audio sent here
/// nor be mistaken for this one's end. With no socket (connect failed or
/// timed out) the samples are discarded instead, so the ring is still drained
/// and the recording always terminates.
async fn serve_utterance(generation: u32, tx_buf: &mut [u8], rx_buf: &mut [u8]) {
    let mut socket = match with_timeout(CONNECT_TIMEOUT, connect_for_upload(tx_buf, rx_buf)).await {
        Ok(socket) => socket,
        Err(_) => {
            print!("ptt: connect timed out, dropping recording\r\n");
            None
        }
    };

    let mut buf = [0i16; SAMPLES_PER_CHUNK];
    loop {
        emit_pending_notices().await;
        // Drain everything this generation has buffered so far. The static
        // ring absorbs (and, when full, drops the oldest of) whatever capture
        // produces meanwhile, so a failed, slow, or congested connection only
        // ever costs buffered audio - it never blocks `capture_task`.
        loop {
            let n = {
                let mut ring = PCM_RING.lock().await;
                ring.read(generation, &mut buf)
            };
            if n == 0 {
                break;
            }
            if let Some(sock) = socket.as_mut()
                && !send_chunk(sock, &buf[..n]).await
            {
                print!("ptt: send failed, dropping rest of recording\r\n");
                socket = None;
            }
        }

        if utterance_ended(
            CURRENT_GEN.load(Ordering::Acquire),
            ENDED_GEN.load(Ordering::Acquire),
            generation,
        ) {
            break;
        }
        // Nothing more for this generation yet; wait for more audio, for the
        // end of the recording, or for a newer recording that supersedes this
        // one (the counters are re-checked on wake).
        select(DATA_READY.wait(), STREAM_ENDED.wait()).await;
    }
    emit_pending_notices().await;

    // `socket` drops here, closing the TCP connection, which is this wire
    // format's end-of-utterance signal to the receiver.
}

#[embassy_executor::task]
async fn ptt_upload_task() {
    let mut tx_buf = [0u8; 2048];
    let mut rx_buf = [0u8; 256];
    let mut next_generation: u32 = 1;

    loop {
        // Serve every utterance exactly once, in order. Generations are
        // contiguous, so once `CURRENT_GEN` has reached one it exists and must
        // be served - even if it was superseded before its connect resolved.
        while next_generation > CURRENT_GEN.load(Ordering::Acquire) {
            UPLOAD_START_SIGNAL.wait().await;
        }
        emit_pending_notices().await;
        serve_utterance(next_generation, &mut tx_buf, &mut rx_buf).await;
        next_generation += 1;
    }
}

/// Resolves and connects to the configured `ptt_host`/`ptt_port`, returning
/// `None` (after logging why) on any failure. `serve_utterance` bounds this
/// with `CONNECT_TIMEOUT`; while it runs, captured samples accumulate in the
/// fixed static ring, which drops the oldest when full, so capture is never
/// blocked by connection setup.
async fn connect_for_upload<'a>(
    tx_buf: &'a mut [u8],
    rx_buf: &'a mut [u8],
) -> Option<TcpSocket<'a>> {
    let Some(stack) = stack().await else {
        print!("ptt: network is offline, dropping recording\r\n");
        return None;
    };

    let (host, port) = {
        let mut config = CONFIG.get().lock().await;
        (
            config.fetch("ptt_host").await,
            config.fetch("ptt_port").await,
        )
    };
    let (Ok(Some(host)), Ok(Some(port))) = (host, port) else {
        print!("ptt: set ptt_host and ptt_port to stream recordings\r\n");
        return None;
    };
    let Ok(port) = port.as_str().parse::<u16>() else {
        print!("ptt: invalid ptt_port `{port}`\r\n");
        return None;
    };

    let dns_client = DnsSocket::new(stack);
    let addr = match dns_client.query(host.as_str(), DnsQueryType::A).await {
        Ok(addrs) if !addrs.is_empty() => addrs[0],
        _ => {
            print!("ptt: failed to resolve {host}\r\n");
            return None;
        }
    };

    let mut socket = TcpSocket::new(stack, tx_buf, rx_buf);
    if let Err(err) = socket.connect(IpEndpoint { addr, port }).await {
        print!("ptt: failed to connect to {host}:{port}: {err:?}\r\n");
        return None;
    }
    Some(socket)
}
