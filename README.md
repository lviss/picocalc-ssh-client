# PicoCalc SSH Client

A standalone SSH client and VT100/ANSI terminal emulator for the [Raspberry Pi Pico 2 W](https://www.raspberrypi.com/products/raspberry-pi-pico-2/) running on the [ClockworkPi PicoCalc](https://www.clockworkpi.com/picocalc).

This project transforms your PicoCalc into a pocket-sized, WiFi-enabled terminal capable of connecting to remote servers via SSH. It is a fork of the [picocalc-wezterm](https://github.com/wez/picocalc-wezterm) project, with improved terminal character support and scrolling. Thus, making the terminal experience more usable.

<p align="center">
  <img src="img/picocalc-ssh-client-gemini.png" width="45%" />
  <img src="img/picocalc-ssh-client-mc.png" width="45%" />
</p>

## Features

*   **Standalone SSH Client**: Connect to any SSH server directly from the device.
*   **Robust Terminal Emulation**: Built on the `vte` crate for accurate ANSI/VT100 parsing.
*   **Extended Character Support**: Custom rendering for box-drawing characters (lines, corners, shades) and common decorative symbols (chevrons, bullets, ellipses, arrows, circles) for TUI applications like `vim`, `gemini-cli`, `claude-code`, `mc`, `htop`, `ollama`, and `tmux`.
*   **Scrolling**: Scroll through the command history, with a heap-budget-derived scrollback limit.
*   **Local Shell**: Built-in commands for device management (WiFi config, battery status, backlight control).
*   **Battery Overlay**: Short-press the power button at any time, even mid-SSH-session, for a brief on-screen battery readout that dismisses itself.
*   **SD Card Key Backup**: Save the SSH private key to the SD card and restore it afterwards, so erasing flash (e.g. `flash_nuke.uf2`) doesn't cost you a freshly generated key and a re-authorisation on every server.
*   **Push-to-Talk Voice Capture**: Hold a button to stream microphone audio to a server-side helper over the SSH session (which transcribes it with Whisper and types it into your tmux pane), or to a configurable TCP host (see below).
*   **Hardware Accelerated**: Uses the RP2350's capabilities and the ILI9488 display for fast rendering.

## Hardware Requirements

*   **ClockworkPi PicoCalc**
*   **Raspberry Pi Pico 2 W** (RP2350 with WiFi)
    *   *Note: This firmware is specifically designed for the RP2350 architecture.*

## Installing the Released Firmware

1. Download the latest firmware from the [releases](https://github.com/richcannings/picocalc-ssh-client/releases) page, like [picocalc-ssh-client.v0.2.uf2](https://github.com/richcannings/picocalc-ssh-client/releases/download/v0.2/picocalc-ssh-client.v0.2.uf2).
2. Flash:
    * Hold the BOOTSEL button on your Pico 2 W while plugging it in.
    * Copy the downloaded firmware, e.g. `picocalc-ssh-client.v0.2.uf2`, to the mounted RP2350 drive.
    * Reboot the Pico 2 W.

## Getting Started with Development

### Prerequisites

You will need a standard Rust toolchain and a few helper tools:

1.  **Install Rust**: [rustup.rs](https://rustup.rs/)
2.  **Install the Nightly Toolchain**:
    ```bash
    rustup toolchain install nightly
    ```
3.  **Add the Compilation Target**:
    ```bash
    rustup target add thumbv8m.main-none-eabihf
    ```
4.  **Install Helper Tools**:
    ```bash
    cargo install flip-link
    # Install picotool (follow instructions at https://github.com/raspberrypi/picotool)
    ```

### Building & Flashing

1.  **Clone the repository**:
    ```bash
    git clone https://github.com/richcannings/picocalc-ssh-client.git
    cd picocalc-ssh-client
    ```

2.  **Build the Firmware**:
    ```bash
    # For Pimoroni Pico Plus 2 W (standard PicoCalc upgrade)
    cargo +nightly build --release --features pimoroni2w
    ```

3.  **Generate UF2 File**:
    ```bash
    # Convert the ELF to UF2
    cp target/thumbv8m.main-none-eabihf/release/picocalc-wezterm target/thumbv8m.main-none-eabihf/release/picocalc-ssh-client.elf
    picotool uf2 convert target/thumbv8m.main-none-eabihf/release/picocalc-ssh-client.elf picocalc-ssh-client.uf2
    ```

4.  **Flash**:
    *   Hold the BOOTSEL button on your Pico 2 W while plugging it in.
    *   Copy `picocalc-ssh-client.uf2` to the mounted `RP2350` drive.

## Usage

### Initial Setup (WiFi)

On first boot, you need to configure your WiFi credentials. The device includes a local shell for configuration.

```bash
# Format the config storage (only needed once)
$ config format

# Set WiFi credentials
$ config set wifi_ssid MyNetwork
$ config set wifi_pw MyPassword

# Reboot to apply
$ reboot
```

> [!CAUTION]
> Credentials (and the SSH private key, if you generate one) are stored
> unencrypted in the device's flash memory.

### Connecting via SSH

Once connected to WiFi (you'll see an IP address), you can connect to a remote host:

```bash
$ ssh mymachine
# or
$ ssh 192.168.1.10
```

Connects to port 22 by default. To use a different port, append it after a
colon, and/or a username before an `@`, using the same syntax as regular
`ssh`:

```bash
$ ssh mymachine:2222
$ ssh myuser@mymachine
$ ssh myuser@mymachine:2222
```

You can also save credentials to avoid typing them every time:

```bash
$ config set ssh_user myuser
$ config set ssh_pw mypassword
```

### Remembering Hosts

Rather than retyping a long hostname (and username/port) every time, save it
under a short alias:

```bash
# Save an alias (accepts the same [user@]host[:port] syntax as `ssh`)
$ ssh save home myuser@myserver.example.com:2222

# Connect using the alias
$ ssh home

# List saved aliases
$ ssh list

# Remove one
$ ssh forget home
```

Aliases are stored in the same flash-backed config as other settings, so
they persist across reboots.

### SSH Key Authentication

The device can generate its own Ed25519 keypair and use it to authenticate,
so you don't need to type (or store) a password at all. The private key is
generated on-device and never leaves it on its own; only the public key needs
to be shared. It can be explicitly exported to an SD card for backup — see
[Backing up and restoring the private key](#backing-up-and-restoring-the-private-key-on-the-sd-card)
below.

```bash
# Generate a keypair (refuses to overwrite an existing one)
$ keygen

# Add the printed "ssh-ed25519 AAAA..." line to the *server's*
# ~/.ssh/authorized_keys file

# Re-display the public key at any time
$ keygen show

# Replace the existing keypair (servers using the old public key
# will need to be updated, or they'll stop accepting it)
$ keygen force
```

Once a key is generated, `ssh` tries it automatically before falling back to
`ssh_pw` or an interactive password prompt.

#### Backing up and restoring the private key on the SD card

Erasing flash (for example `flash_nuke.uf2` before switching the PicoCalc to
another firmware) also wipes the config sectors that hold the private key,
which would otherwise mean generating a new key and re-authorising it on
every server. With an SD card inserted, the key can be written to the card
and restored later:

```bash
# Write the current private key to the card (creates ssh_key.hex;
# refuses if a backup already exists)
$ keygen save

# Overwrite an existing backup on the card
$ keygen save force

# Restore a key from the card (refuses if the device already has a key)
$ keygen load

# Replace the device's current key with the one on the card
$ keygen load force
```

*   **File and location**: `ssh_key.hex` in the root directory of the card's
    first (FAT) partition. FAT short names are stored upper-cased, so the file
    appears as `SSH_KEY.HEX` in a PC card reader (and in `ls`).
*   **Format**: exactly the 64-character hex string the config store already
    holds under `ssh_key` — plain text, nothing else in the file, so it can be
    inspected or copied with any editor. A trailing newline is tolerated when
    reading.
*   **Safety**: `keygen save` needs a key to exist (run `keygen` first) and
    will not overwrite an existing backup without `force`; `keygen load` will
    not replace an existing on-device key without `force`, exactly like
    `keygen` itself. Every failure — no card, missing or malformed file, a
    failed write — prints a reason instead of doing nothing.

> [!CAUTION]
> `keygen save` writes the **private key in plain text** to removable media.
> The card then has to be treated as a secret in its own right: anyone who can
> read it can impersonate the device on every server that authorises the
> matching public key. Keep the card somewhere safe and delete `ssh_key.hex`
> when you no longer need the backup. The export is always explicit — the
> firmware never copies the key to the card on its own, and it never loads a
> key from the card automatically at boot either.

#### Public key on the SD card

Unlike the private key, the public key doesn't need an explicit export step.
Whenever it's generated or shown (`keygen`, `keygen force`, or `keygen show`),
it's also mirrored to `ssh_key.pub` in the root directory of the card's first
(FAT) partition — the same `ssh-ed25519 <base64> picocalc-ssh-client` line
printed to the LCD and serial console, ready to paste into a server's
`~/.ssh/authorized_keys`. FAT short names are stored upper-cased, so the file
appears as `SSH_KEY.PUB` in a PC card reader (and in `ls`). This write always
overwrites whatever was there — the public key isn't a secret, so there's no
`force`/overwrite-protection to worry about, and a stale mismatched copy on
the card would only be confusing. It's also best-effort: if no card is
present or the write fails, the console prints one line saying so and the
`keygen`/`keygen show` command still succeeds — the LCD/serial output is
never blocked on it.

**How to test:**

1.  With an SD card inserted, run `keygen force` (or `keygen show` if a key
    already exists) on the device console.
2.  Confirm the console prints both the `ssh-ed25519 AAAA... picocalc-ssh-client`
    line and a `Mirrored the public key to ssh_key.pub ...` confirmation line.
3.  Pull the card and read it on another machine — `cat ssh_key.pub` (or open
    `SSH_KEY.PUB` in any text editor) — and check the line matches exactly
    what the console printed.
4.  Re-insert the card, run `keygen show` again, and confirm the file's
    contents are unchanged (same key, rewritten in place).
5.  Remove the card and run `keygen show`; confirm the public key still
    prints normally and the console instead prints a
    `No SD card is present; not mirroring the public key ...` line, with the
    command otherwise succeeding.

#### Recovering after a flash erase

1.  Flash the firmware as usual (BOOTSEL, copy the `.uf2`, reboot).
2.  Insert the SD card that holds `SSH_KEY.HEX`.
3.  Run `keygen load`. It prints the public key it just restored.
4.  Run `keygen show` and check the `ssh-ed25519 AAAA...` line matches the one
    already in your servers' `~/.ssh/authorized_keys`; your existing
    authorisations keep working, with no need to re-authorise anything.

The erase clears *every* stored setting, not just the key: WiFi credentials,
`ssh_user`/`ssh_pw`, saved `ssh` aliases, `scroll`, and — in builds that have
it — the push-to-talk `ptt_*` settings all have to be re-entered with
`config set`. Only the SSH key has a save/restore path today; the same
file-on-the-card approach could carry the rest of the config, but that isn't
implemented here.

#### Retrieving the public key

The public key line is long (an Ed25519 `ssh-ed25519 AAAA...` line is around
100 characters), too long to reliably copy by hand off the LCD. With an SD
card inserted, the easiest way is to just read it off the card — see
[Public key on the SD card](#public-key-on-the-sd-card) above; no cable
needed. Without a card, retrieve it over the device's USB serial log port
instead:

1. Connect a USB cable to the PicoCalc (the same port used to flash it, once
   it's booted normally rather than in BOOTSEL mode).
2. Open a serial terminal on that port. Any baud rate works, since it's a
   USB-CDC virtual serial port, not a real UART:
   * **Linux**: `dmesg | tail` after plugging in to find the device (usually
     `/dev/ttyACM0`), then `screen /dev/ttyACM0 115200`.
   * **macOS**: `ls /dev/tty.usbmodem*`, then `screen /dev/tty.usbmodem* 115200`.
   * **Windows**: check Device Manager → Ports for the new COM port, then
     open it in PuTTY (connection type "Serial", any speed) or a similar
     terminal.
3. On the PicoCalc, run `keygen show`. The `ssh-ed25519 AAAA... picocalc-ssh-client`
   line is printed to that serial terminal, where it can be copied exactly
   (unlike the wrapped text on the LCD) and pasted into the server's
   `~/.ssh/authorized_keys`.

### Scrolling

You can scroll through the command history using the following key combinations:

*   `Ctrl + UpArrow`: Scroll up
*   `Ctrl + DownArrow`: Scroll down

Typing any character or receiving new output from the server will automatically reset the view to the bottom.

You can configure the number of lines in the scrollback buffer. The maximum
(and default) is derived from the device's available heap and current screen
geometry rather than a fixed number, so it varies by device/font; check the
current value with `config get scroll`:

```bash
$ config get scroll
$ config set scroll 100  # must be <= the heap-budget limit reported on error
$ config rm scroll  # Resets to the heap-budget default
```

### Battery Overlay

A short press of the physical power button shows a bordered "Battery: NN%"
box centered on screen for a few seconds, then it disappears on its own. It
works at any time, including in the middle of an active SSH session, and
never disturbs the underlying screen content — whatever was there (or
arrives from the remote host while the overlay is up) is exactly what's
shown once it clears.

Holding the power button down instead powers off the device; that's handled
entirely by the keyboard co-processor and doesn't involve this firmware.

### Push-to-Talk Voice Capture

Hold `F1` (plain, no modifiers - see `src/keyboard.rs` if you want to rebind
it to a different key) to capture microphone audio. Where it goes depends on
whether an SSH session is up:

*   With an SSH session active, the audio travels down that *same* connection
    to a companion process on the server, which transcribes it and types the
    text into your tmux session. This is the transport to use for dictation,
    and it needs no configuration on the device at all.
*   Otherwise it is streamed to a plain TCP host you configure with
    `ptt_host`/`ptt_port`, as it was before dictation existed.

#### Transcribing into your tmux session

While an SSH session is connected, `F1` opens a *second SSH channel* on that
same connection, runs `tools/picocalc-ptt` on it, and streams the audio
there:

```
PicoCalc --(ssh terminal channel)------------------------> your shell in tmux
         --(ssh audio channel: exec picocalc-ptt)--> whisper --> tmux send-keys
```

Nothing new listens on the server, and nothing new is authenticated: the audio
rides the connection the device already has, and the helper runs as you, with
your permissions, exactly as your shell does. A port forward was the
alternative, but either direction needs a listening socket (on the server, or
on the device), and this firmware's SSH stack (`sunset`) implements no
forwarding at all - so a channel on the session is both the lighter change and
the smaller security surface.

To set it up, copy `tools/picocalc-ptt` to the server, put it somewhere on
your non-interactive `PATH` (for example `/usr/local/bin`), make it
executable, and point it at whisper in `~/.config/picocalc-ptt.conf`:

```ini
# ~/.config/picocalc-ptt.conf
whisper = whisper-cli -m ~/models/ggml-base.en.bin -f %wav -nt -np
# or, with openai-whisper:
# whisper = whisper %wav --model base --output_format txt --output_dir %dir
tmux_target = work     # optional; default is tmux's most recently used session
enter = no             # yes to also press Enter after typing
```

`%wav` is replaced by the capture's WAV path and `%dir` by a private working
directory. The transcript is read from the command's standard output, or from
`%dir/picocalc-ptt.txt` when it printed nothing there - which is what both
whisper.cpp's `-of` and openai-whisper's `--output_dir` write. The text is then
typed into the target pane literally, with a trailing space (so consecutive
dictations do not run together) and no newlines.

Then hold `F1` and speak. The device runs `picocalc-ptt` as-is, so a different
path or extra options go in `ptt_ssh_cmd`:

```bash
$ config set ptt_ssh_cmd "/home/me/bin/picocalc-ptt --tmux-target work"
$ config get ptt_ssh_cmd        # what the next session will run
$ config set ptt_ssh_cmd ""     # empty disables it: use the raw TCP host instead
```

If the helper cannot be started or it exits, the device reports why on screen
(its stderr is shown) and falls back to the raw TCP host for the rest of that
session. The helper needs Python 3 plus your whisper command, and nothing else.
Run `tools/picocalc-ptt --help` for its options and
`python3 tools/test_picocalc_ptt.py` for its tests, which need neither whisper
nor tmux.

#### Streaming to a raw TCP host

For capturing to a receiver of your own (a laptop on the same network, another
transcription setup), disable the SSH helper and configure the destination:

```bash
$ config set ptt_ssh_cmd ""
$ config set ptt_host mymachine.example.com
$ config set ptt_port 9000
```

The receiver gets one TCP connection per utterance, opened when you press
`F1` and closed when you release it: a 4-byte little-endian byte count, then
that many bytes of audio, repeated. By default the audio is mono 16-bit
little-endian PCM at `ptt_rate` (16 kHz), which the wire does not signal, so
the receiver has to be told the rate out of band; see AGENTS.md for the
authoritative contract.

#### Microphone bring-up and diagnostics

For bringing up a new microphone there are also optional debug settings. They
take effect on the *next* recording without a rebuild or reflash, and default to
the documented-correct values:

```bash
$ config set ptt_bits 32     # I2S channel slot width in bits (default 32)
$ config set ptt_rate 16000  # sample rate in Hz (default 16000)
$ config set ptt_edge 1      # invert the BCLK sampling edge (default 0)
$ config set ptt_raw 1       # stream raw FIFO words instead of PCM (default 0)
$ config set ptt_gain 256    # x256 DC-removed capture gain, 1-4096 (default 1)
```

`ptt_gain` is a diagnostic for a very quiet microphone: it subtracts each
capture chunk's DC offset and then amplifies what is left (saturating, so it
clips rather than wraps), in both the PCM and `ptt_raw` paths. A gain of `1`
(the default) does no DC removal and no scaling, so an unconfigured device is
byte-for-byte unchanged. It cannot conjure a signal that is not there - it only
makes a faint one easier to see.

The microphone has no hardware gain register (its `SEL` pin only selects the
left/right slot), so `ptt_gain` is explicitly a **diagnostic**: real gain and
normalization belong in the receiving/transcription pipeline, where the audio is
consumed, and the device ships its native sample levels. While recording, the
overlay also draws a realtime input meter under "recording..." - it shows a
windowed DC-removed level (a median across the chunk's sub-windows), so a mic
sitting on its noise floor reads empty and sustained speech fills the bar (it
turns red if the input is pinned) - making "hold `F1` and speak" the quickest
"is the mic hearing anything?" check. Note: a separate, real per-chunk capture
glitch (a few extreme samples once per 25 ms DMA transfer) is still present in
the streamed audio; the windowed median keeps it from dominating the meter,
but it is a firmware/I2S-path defect that remains open. The mic's audio path
is proven to carry real speech (verified against real captures independently
transcribable by openai-whisper and whisper.cpp) - see `AGENTS.md` for the
capture analysis and the still-open per-chunk glitch.

`ptt_bits`/`ptt_rate` must keep the resulting bit clock (`rate * bits * 2`)
inside the SPH0645's documented 1.024-4.096 MHz window; an out-of-window pair is
refused with the allowed range. `config get` and `config list` report the value
the next recording will actually use (a stored key that no longer matches is
rewritten so store, report, and effective value agree; the slot width and rate
are rewritten together, so a partially failed repair cannot clock the mic
differently than reported), and `config rm`
restores the default. If that rewrite cannot land (e.g. the config region is
full), the console flags the key as `unreconciled` alongside the stale value the
store still holds, and a `config set` that would make that stale value effective
again is refused rather than silently changing the reported setting.

A small "recording..." overlay is shown while the button is held.
Transcription is not implemented by this firmware — it only captures and
streams raw audio; see AGENTS.md for the wire format a receiving process
needs to speak, README-DEVICE.md for the mic's I2S pin wiring, and
`tools/picocalc-ptt` for the transcribing end of the SSH transport.

> [!NOTE]
> This requires a digital I2S microphone wired to the pins documented in
> README-DEVICE.md. The SSH transport is encrypted, being part of the SSH
> session; the `ptt_host`/`ptt_port` fallback streams audio unencrypted over a
> plain TCP connection, so only use that on a network you trust.

### Local Commands

*   `cls`: Clear the screen.
*   `bat`: Show battery status.
*   `bl lcd <percent>`: Set LCD backlight brightness (e.g., `bl lcd 50`).
*   `bl kbd <percent>`: Set keyboard backlight brightness (requires updated keyboard firmware).
*   `free`: Show memory usage.
*   `bootsel`: Reboot into bootloader mode.
*   `keygen [force|show]`: Generate (or re-display) an SSH keypair for public-key authentication. Also mirrors the public key to `ssh_key.pub` on the SD card if one is present (see [Public key on the SD card](#public-key-on-the-sd-card)).
*   `keygen save [force]`: Write the private key to `ssh_key.hex` on the SD card (see [Backing up and restoring the private key](#backing-up-and-restoring-the-private-key-on-the-sd-card)).
*   `keygen load [force]`: Restore the private key from `ssh_key.hex` on the SD card.

## Credits

*   Forked from [wezterm/picocalc-wezterm](https://github.com/wezterm/picocalc-wezterm).
*   Original SSH implementation using [sunset](https://github.com/wez/sunset).
*   Terminal emulation powered by [vte](https://github.com/alacritty/vte).
