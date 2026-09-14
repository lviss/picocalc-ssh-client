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
*   **Push-to-Talk Voice Capture**: Hold a button to stream microphone audio to a configurable network host (see below) for off-device transcription.
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
100 characters), too long to reliably copy by hand off the LCD. Instead,
retrieve it over the device's USB serial log port:

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
it to a different key) to capture microphone audio and stream it to a
network host of your choice — for
example, a companion process on your SSH server that runs speech-to-text and
injects the resulting text into your session. Configure the destination
before using it:

```bash
$ config set ptt_host mymachine.example.com
$ config set ptt_port 9000
```

For bringing up a new microphone there are also optional debug settings. They
take effect on the *next* recording without a rebuild or reflash, and default to
the documented-correct values:

```bash
$ config set ptt_bits 32     # I2S channel slot width in bits (default 32)
$ config set ptt_rate 16000  # sample rate in Hz (default 16000)
$ config set ptt_edge 1      # invert the BCLK sampling edge (default 0)
$ config set ptt_raw 1       # stream raw FIFO words instead of PCM (default 0)
```

`ptt_bits`/`ptt_rate` must keep the resulting bit clock (`rate * bits * 2`)
inside the SPH0645's documented 1.024-4.096 MHz window; an out-of-window pair is
refused with the allowed range. `config get ptt_bits` (and friends) reports the
value the next recording will actually use, and `config rm` restores the
default.

A small "recording..." overlay is shown while the button is held.
Transcription itself is not implemented by this firmware — it only captures
and streams raw audio; see AGENTS.md for the wire format a receiving process
needs to speak, and README-DEVICE.md for the mic's I2S pin wiring.

> [!NOTE]
> This requires a digital I2S microphone wired to the pins documented in
> README-DEVICE.md. It streams audio unencrypted over a plain TCP
> connection; only use it on a network you trust.

### Local Commands

*   `cls`: Clear the screen.
*   `bat`: Show battery status.
*   `bl lcd <percent>`: Set LCD backlight brightness (e.g., `bl lcd 50`).
*   `bl kbd <percent>`: Set keyboard backlight brightness (requires updated keyboard firmware).
*   `free`: Show memory usage.
*   `bootsel`: Reboot into bootloader mode.
*   `keygen [force|show]`: Generate (or re-display) an SSH keypair for public-key authentication.
*   `keygen save [force]`: Write the private key to `ssh_key.hex` on the SD card (see [Backing up and restoring the private key](#backing-up-and-restoring-the-private-key-on-the-sd-card)).
*   `keygen load [force]`: Restore the private key from `ssh_key.hex` on the SD card.

## Credits

*   Forked from [wezterm/picocalc-wezterm](https://github.com/wezterm/picocalc-wezterm).
*   Original SSH implementation using [sunset](https://github.com/wez/sunset).
*   Terminal emulation powered by [vte](https://github.com/alacritty/vte).
