//! Push-to-talk voice capture: a PIO-driven I2S RX driver for a digital mic
//! wired to the pins freed by removing SD card support (see AGENTS.md), plus
//! the task that streams captured audio to a configurable network host.
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
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::pin::pin;
use core::sync::atomic::{AtomicBool, Ordering};
use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_net::IpEndpoint;
use embassy_net::dns::{DnsQueryType, DnsSocket};
use embassy_net::tcp::TcpSocket;
use embassy_rp::PeripheralRef;
use embassy_rp::clocks::clk_sys_freq;
use embassy_rp::peripherals::{DMA_CH4, PIN_16, PIN_17, PIN_18, PIO2};
use embassy_rp::pio::program::pio_asm;
use embassy_rp::pio::{Config, Direction, FifoJoin, Pio, ShiftConfig, ShiftDirection, StateMachine};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use embedded_io_async::Write as _;
use fixed::traits::ToFixed;

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
/// Cap on `ptt_upload_task`'s `pending` backlog (~1.6s of audio at 25ms per
/// chunk): comfortably above realistic DNS+TCP-connect latency, so a
/// slow/unreachable `ptt_host` degrades to bounded audio loss (oldest
/// buffered chunks dropped) instead of unbounded heap growth while the
/// button stays held.
const MAX_PENDING_CHUNKS: usize = 64;

type PcmChunk = Box<[i16; SAMPLES_PER_CHUNK]>;

enum StreamMsg {
    Chunk(PcmChunk),
    End,
}

/// The PSRAM-backed elastic buffer absorbing scheduling jitter between the
/// capture and network-send tasks (see AGENTS.md's heap/PSRAM notes): the
/// `Box<[i16; _]>` payloads are ordinary heap allocations, which the global
/// `DualHeap` overflows into PSRAM once the tiny primary heap is full.
/// Access is guarded by this channel's internal critical-section mutex, not
/// atomics, per `heap.rs`'s "PSRAM isn't CAS-atomic-safe" caveat.
static STREAM: Channel<CriticalSectionRawMutex, StreamMsg, 4> = Channel::new();

static RECORDING: AtomicBool = AtomicBool::new(false);
static START_SIGNAL: Signal<CriticalSectionRawMutex, ()> = Signal::new();
/// Separate from `START_SIGNAL` (each `Signal` has exactly one waiter:
/// `capture_task` waits on `START_SIGNAL`, `ptt_upload_task` waits on this
/// one) so the network connection can be dialed concurrently with the very
/// first captured chunks instead of only after `ptt_upload_task` observes
/// one on `STREAM` — see AGENTS.md's note on `STREAM`'s capacity being
/// smaller than realistic connection-setup latency.
static UPLOAD_START_SIGNAL: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Begins push-to-talk capture; a no-op if already recording. Called from
/// `keyboard.rs` on `(KeyState::Pressed, Key::ButtonLeft2)`.
pub async fn start_recording() {
    if !RECORDING.swap(true, Ordering::AcqRel) {
        SCREEN
            .get()
            .lock()
            .await
            .show_overlay(String::from("recording..."));
        START_SIGNAL.signal(());
        UPLOAD_START_SIGNAL.signal(());
    }
}

/// Ends push-to-talk capture; a no-op if not recording. Called from
/// `keyboard.rs` on `(KeyState::Released, Key::ButtonLeft2)`.
pub async fn stop_recording() {
    if RECORDING.swap(false, Ordering::AcqRel) {
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
        self.sm.rx().dma_pull(self.dma_ch.reborrow(), buf, false).await;
    }
}

/// Claims PIO2 (unclaimed elsewhere in this codebase; PIO0 is WiFi, PIO1 is
/// PSRAM) and the pins freed by dropping SD card support, and spawns the
/// capture and network-upload tasks. `bclk`/`ws`/`sd` must be
/// `PIN_16`/`PIN_17`/`PIN_18` per AGENTS.md's pin contract.
pub fn init_mic(spawner: &Spawner, pio2: PIO2, bclk: PIN_16, ws: PIN_17, sd: PIN_18, dma_ch4: DMA_CH4) {
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
        mic.set_enabled(true);
        while RECORDING.load(Ordering::Acquire) {
            let mut raw = [0u32; SAMPLES_PER_CHUNK];
            mic.capture(&mut raw).await;

            // One 32-bit FIFO word per L+R frame (ShiftDirection::Left, so
            // MSB-first): the mic's 16-bit sample lands in the upper half
            // when wired to the left slot (WS low), matching this driver's
            // pin/wiring contract in AGENTS.md. If a captain instead wires
            // the mic to the right slot, swap this to the lower 16 bits.
            let mut pcm: PcmChunk = Box::new([0i16; SAMPLES_PER_CHUNK]);
            for (dst, word) in pcm.iter_mut().zip(raw.iter()) {
                *dst = (*word >> 16) as i16;
            }
            STREAM.send(StreamMsg::Chunk(pcm)).await;
        }
        mic.set_enabled(false);
        STREAM.send(StreamMsg::End).await;
    }
}

async fn send_chunk(socket: &mut TcpSocket<'_>, chunk: &[i16]) -> bool {
    let mut bytes = Vec::with_capacity(4 + chunk.len() * 2);
    bytes.extend_from_slice(&(chunk.len() as u32 * 2).to_le_bytes());
    for sample in chunk {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    socket.write_all(&bytes).await.is_ok()
}

/// Resolves and connects to the configured `ptt_host`/`ptt_port`, returning
/// `None` (after logging why) on any failure. `ptt_upload_task` races this
/// against continued `STREAM.receive()` calls (rather than awaiting it to
/// completion first) so the small `STREAM` channel keeps draining — and
/// `capture_task`'s DMA pulls / I2S clock keep running — for the whole
/// DNS+connect window instead of only after it.
async fn connect_for_upload<'a>(tx_buf: &'a mut [u8], rx_buf: &'a mut [u8]) -> Option<TcpSocket<'a>> {
    let Some(stack) = stack().await else {
        print!("ptt: network is offline, dropping recording\r\n");
        return None;
    };

    let (host, port) = {
        let mut config = CONFIG.get().lock().await;
        (config.fetch("ptt_host").await, config.fetch("ptt_port").await)
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

#[embassy_executor::task]
async fn ptt_upload_task() {
    loop {
        UPLOAD_START_SIGNAL.wait().await;

        let mut tx_buf = [0u8; 2048];
        let mut rx_buf = [0u8; 256];

        // Chunks that arrive on `STREAM` while `connect_for_upload` is
        // still resolving DNS + TCP connect below: buffered here (instead
        // of leaving them queued on the bounded `STREAM` channel) so
        // `capture_task` never blocks on a full channel during connection
        // setup. Sent once the socket is ready, in the same order.
        let mut pending: Vec<PcmChunk> = Vec::new();
        let mut ended = false;

        let mut socket = {
            let mut connect_fut = pin!(connect_for_upload(&mut tx_buf, &mut rx_buf));
            let mut dropped_pending = false;
            loop {
                match select(&mut connect_fut, STREAM.receive()).await {
                    Either::First(socket) => break socket,
                    Either::Second(StreamMsg::Chunk(chunk)) => {
                        if pending.len() >= MAX_PENDING_CHUNKS {
                            pending.remove(0);
                            if !dropped_pending {
                                print!("ptt: connect is slow, dropping oldest buffered audio\r\n");
                                dropped_pending = true;
                            }
                        }
                        pending.push(chunk);
                    }
                    Either::Second(StreamMsg::End) => {
                        ended = true;
                        break None;
                    }
                }
            }
        };

        if ended && !pending.is_empty() {
            print!("ptt: recording ended before connecting, dropping buffered audio\r\n");
        }

        for chunk in pending {
            if let Some(sock) = socket.as_mut()
                && !send_chunk(sock, chunk.as_slice()).await
            {
                print!("ptt: send failed, dropping rest of recording\r\n");
                socket = None;
            }
        }

        // Dropping `socket` once this loop exits on `StreamMsg::End` closes
        // the TCP connection, which is this wire format's end-of-utterance
        // signal to the receiver.
        if !ended {
            while let StreamMsg::Chunk(chunk) = STREAM.receive().await {
                if let Some(sock) = socket.as_mut()
                    && !send_chunk(sock, chunk.as_slice()).await
                {
                    print!("ptt: send failed, dropping rest of recording\r\n");
                    socket = None;
                }
            }
        }
    }
}
