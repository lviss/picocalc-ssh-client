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
//! Wire format (see AGENTS.md for the authoritative copy of this contract):
//! one TCP connection per utterance (opened on button press, closed on
//! release). Each captured frame is sent as a 4-byte little-endian u32 byte
//! count, followed by that many bytes of raw signed 16-bit little-endian
//! mono PCM samples at 16 kHz. No handshake and no other framing.

use crate::Irqs;
use crate::config::CONFIG;
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
use embassy_rp::pio::program::pio_asm;
use embassy_rp::pio::{
    Config, Direction, FifoJoin, Pio, ShiftConfig, ShiftDirection, StateMachine,
};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, with_timeout};
use embedded_io_async::Write as _;
use fixed::traits::ToFixed;
use terminal_model::audio_ring::{AudioRing, utterance_ended};

extern crate alloc;

/// Matches Whisper's internal resampling target, so there's no benefit to
/// capturing at a higher rate (see the feasibility report referenced in
/// AGENTS.md).
const SAMPLE_RATE_HZ: u32 = 16_000;
const BIT_DEPTH: u32 = 16;
/// I2S always frames a left+right pair per word-select cycle even though
/// this mono mic only drives one slot; see `capture_task`'s extraction.
const CHANNELS: u32 = 2;
/// 25ms per frame, comfortably inside the brief's 20-50ms guidance.
const SAMPLES_PER_CHUNK: usize = SAMPLE_RATE_HZ as usize / 40;
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
struct Mic {
    sm: StateMachine<'static, PIO2, 0>,
    dma_ch: PeripheralRef<'static, DMA_CH4>,
}

impl Mic {
    fn set_enabled(&mut self, enabled: bool) {
        self.sm.set_enable(enabled);
    }

    async fn capture(&mut self, buf: &mut [u32]) {
        self.sm
            .rx()
            .dma_pull(self.dma_ch.reborrow(), buf, false)
            .await;
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

    // Mirror of PioI2sOutProgram's pio_asm! block: same word/bit-clock
    // side-set generation (the Pico is I2S master either way), with `in
    // pins, 1` replacing `out pins, 1` to capture instead of emit.
    let prg = pio_asm!(
        ".side_set 2",
        "    set x, 14          side 0b01", // side 0bWB - W = Word Clock, B = Bit Clock
        "left_data:",
        "    in pins, 1         side 0b00",
        "    jmp x-- left_data  side 0b01",
        "    in pins, 1         side 0b10",
        "    set x, 14          side 0b11",
        "right_data:",
        "    in pins, 1         side 0b10",
        "    jmp x-- right_data side 0b11",
        "    in pins, 1         side 0b00",
    );
    let program = pio.common.load_program(&prg.program);

    let bit_clock_pin = pio.common.make_pio_pin(bclk);
    let lr_clock_pin = pio.common.make_pio_pin(ws);
    let data_pin = pio.common.make_pio_pin(sd);

    let mut cfg = Config::default();
    cfg.use_program(&program, &[&bit_clock_pin, &lr_clock_pin]);
    cfg.set_in_pins(&[&data_pin]);
    let clock_frequency = SAMPLE_RATE_HZ * BIT_DEPTH * CHANNELS;
    cfg.clock_divider = (clk_sys_freq() as f64 / clock_frequency as f64 / 2.).to_fixed();
    cfg.shift_in = ShiftConfig {
        threshold: 32,
        direction: ShiftDirection::Left,
        auto_fill: true,
    };
    // Doubles RX FIFO depth since TX is unused; mirrors PioI2sOut's TxOnly.
    cfg.fifo_join = FifoJoin::RxOnly;

    let mut sm = pio.sm0;
    sm.set_config(&cfg);
    sm.set_pin_dirs(Direction::In, &[&data_pin]);
    sm.set_pin_dirs(Direction::Out, &[&bit_clock_pin, &lr_clock_pin]);
    // Left disabled (no clocks driven, no mic power/noise) until a
    // recording actually starts.
    sm.set_enable(false);

    let mic = Mic {
        sm,
        dma_ch: PeripheralRef::new(dma_ch4),
    };

    spawner.must_spawn(capture_task(mic));
    spawner.must_spawn(ptt_upload_task());
}

#[embassy_executor::task]
async fn capture_task(mut mic: Mic) {
    loop {
        START_SIGNAL.wait().await;
        let generation = CURRENT_GEN.load(Ordering::Acquire);
        mic.set_enabled(true);
        let started = Instant::now();
        // Ends when the button is released, when a newer recording supersedes
        // this one, or on the recording-duration cap below.
        while RECORDING.load(Ordering::Acquire) == generation
            && CURRENT_GEN.load(Ordering::Acquire) == generation
        {
            let mut raw = [0u32; SAMPLES_PER_CHUNK];
            mic.capture(&mut raw).await;
            if CURRENT_GEN.load(Ordering::Acquire) != generation {
                continue;
            }

            // One 32-bit FIFO word per L+R frame (ShiftDirection::Left, so
            // MSB-first): the mic's 16-bit sample lands in the upper half
            // when wired to the left slot (WS low), matching this driver's
            // pin/wiring contract in AGENTS.md. If a captain instead wires
            // the mic to the right slot, swap this to the lower 16 bits.
            let mut pcm = [0i16; SAMPLES_PER_CHUNK];
            for (dst, word) in pcm.iter_mut().zip(raw.iter()) {
                *dst = (*word >> 16) as i16;
            }

            // Never blocks and never allocates: if the network side is
            // behind, the ring drops its oldest samples instead of stalling
            // the DMA pull that keeps the I2S clocks running.
            let result = PCM_RING.lock().await.write(generation, &pcm);
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
