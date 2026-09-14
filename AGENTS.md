# Project agent memory

This file is the project's committed home for project-intrinsic agent knowledge: build, test, release, architecture, and sharp-edge notes that should travel with the code.

- Add durable project-specific notes here as they are discovered through real work.

## Build and test

- This is a `no_std`/`no_main` firmware crate for `thumbv8m.main-none-eabihf` (Pico2W/Pimoroni2W).
  `make check` / `make image` (see `Makefile`) select the chip via `--features pico2w` or
  `--features pimoroni2w` — the crate does not build at all with no chip feature selected.
  CI (`.github/workflows/build.yml`) only runs `make image`; there is no `cargo test` step.
- Most of this crate's dependencies (embassy-rp, cyw43, mipidsi, ...) are real hardware/PAC
  bindings and will not compile for a host target — don't try to `cargo test`/`cargo check` the
  root `picocalc-wezterm` package for `x86_64-unknown-linux-gnu`, it fails deep in `embassy-rp`.
- `terminal-model/` is a separate workspace-member crate (path dependency) holding the
  hardware-independent logic the firmware pulls in: terminal buffer/VTE (`screen_model.rs`) and
  vector glyph-drawing (`glyphs.rs`) from `src/screen.rs`, push-to-talk key dispatch
  (`key_dispatch.rs`) from `src/keyboard.rs`, and the SD-card SSH-key backup text codec
  (`keyfile.rs`) from `src/sshkey.rs`. It depends only on `vte`, `embedded-graphics`, and
  `profont` — all host-buildable — so it's the place for real, runnable unit tests. Run them with
  `cargo test -p terminal-model --target x86_64-unknown-linux-gnu` (must override the default
  target set in `.cargo/config.toml`). If new logic needs a host test and doesn't fit here, prefer
  extending this crate over adding tests to the hardware-coupled root crate.
- `.github/workflows/build.yml` has four jobs. On `pull_request`: `determine-version` (parses the
  prior PR comment's `v<N>` heading, made by the job below, to compute the next per-PR iteration
  number — starts at 1, deliberately not `github.run_number`/`run_attempt` since neither is a
  per-PR counter) → `build-pr` (matrix over `pico2w`/`pimoroni2w`, builds via `make CHIP=<chip>
  image`, uploads each as a workflow artifact named `picocalc-ssh-client-<chip>-pr<N>-v<version>`)
  → `comment-pr` (needs `build-pr`, runs once — not matrixed — and posts/updates a single PR
  comment linking both chips' just-built artifacts, via `actions/github-script` listing this run's
  artifacts over the REST API rather than passing `upload-artifact`'s `artifact-id`/`artifact-url`
  step outputs through matrix job outputs: GitHub Actions matrix job outputs are last-write-wins
  per key across all matrix legs unconditionally, even when the losing leg's value was an empty/
  guard-conditioned string, so a naive "one output key per chip" scheme silently drops one chip's
  link depending on leg completion order — the artifact-listing API sidesteps that race). The
  comment is found and edited in place on later pushes via a stable `<!-- picocalc-uf2-artifacts
  -->` HTML marker, not reposted. `build-release`/`publish-release` (manual `workflow_dispatch`
  with a required `version` input, builds both chips, publishes one GitHub Release tagged
  `version` with both `.uf2` assets) are unrelated to the PR flow. There is no push-to-main
  auto-release anymore.
  This repo is a fork, and GitHub disables Actions by default on forks until the owner opts in from
  the repo's web UI Settings > Actions page (the REST `actions/permissions` endpoint 403s for a
  non-admin token, so this can't be done via `gh api`) — check `gh api
  repos/lviss/picocalc-ssh-client/actions/runs` for a nonzero run count before assuming CI is live.
- `vte::Params` always pushes a parameter slot on `csi_dispatch`, even for a bare sequence with no
  digits (e.g. `CSI C`) — the "no parameter" case arrives as an explicit `0`, not an absent one.
  A plain `params.iter().next().map(|p| p[0]).unwrap_or(1)` therefore silently computes `0` instead
  of the ECMA-48 default of `1` for the extremely common bare-sequence form. Use
  `.unwrap_or(0).max(1)` (see `cursor_move_count` in `terminal-model/src/screen_model.rs`) for any
  CSI parameter that has a nonzero default.
- `ScreenModel::overlay` (`terminal-model/src/screen_model.rs`) is the pattern for any transient
  on-screen banner (currently just the battery readout on a power-button press, wired in
  `src/keyboard.rs`'s `keyboard_reader`): it's a paint-time-only flag that never touches
  `lines`/`scrollback`, composited on top each frame in `src/screen.rs`'s `update_display` /
  `draw_overlay`. `clear_overlay()` forces `full_repaint = true` so dismissal redraws the real,
  possibly-changed cell content underneath from scratch rather than needing a save/restore buffer.
  Auto-dismiss timing (`embassy_time::Instant`) lives on the `Screen` wrapper in `src/screen.rs`
  (`overlay_expiry`, checked in `Screen::update_display`), not in `ScreenModel`, since
  `terminal-model` is host-portable and has no clock.
- Despite the caution above about the root package not building for the host target: this repo's
  installed toolchain does carry a prebuilt `thumbv8m.main-none-eabihf` std, so
  `cargo check --features pimoroni2w` (or `pico2w`) on the root package works and fully
  type-checks the firmware crate — useful for validating non-`terminal-model` changes without
  hardware. `cargo build --release --features <chip>` (what `make image` runs) also compiles all
  the way through codegen and linking with `flip-link` on `PATH` (in this sandbox it's already
  installed at `/home/ai/.cargo/bin/flip-link`, just not on `PATH` by default — `cargo install
  flip-link` is a no-op confirming this; add `/home/ai/.cargo/bin` to `PATH` rather than
  reinstalling). If `flip-link` is genuinely absent and can't be installed (no network/build
  tools), that's a linker availability gap, not a code problem, if it's the only failure.
- `terminal-model::screen_model`'s `ScreenModel::max_scrollback` is not a flat literal - it's
  computed by `safe_max_scrollback_for(cols, rows)` against `SCREEN_HEAP_BUDGET_BYTES`
  (`FIRMWARE_HEAP_SIZE_BYTES` minus `NON_SCREEN_HEAP_RESERVE_BYTES`, the heap WiFi/TCP/SSH/SD and
  other boot-time subsystems reliably need per real-hardware `free`-command readings). A correctly
  *capped* scrollback buffer can still exceed the primary heap by 2x+ if the cap is a flat number
  disconnected from actual heap size - see `/ai/firstmate/data/picocalc-crash-display-buffer/report.md`
  and the two host tests next to `safe_max_scrollback_for` in `terminal-model/src/screen_model.rs`
  (`default_scrollback_cap_keeps_full_footprint_within_heap_budget`,
  `heavy_output_scroll_pressure_accelerates_once_visible_area_fills_up` - the latter documents why
  memory pressure from heavy output *accelerates* rather than growing linearly: `scroll_up()` only
  allocates a new `ScreenLine` once the visible `lines` area is already full, so output that still
  fits on-screen is nearly free while output that has to scroll costs one full line-allocation per
  line). `ScreenModel::set_max_scrollback` self-clamps to `max_safe_scrollback()`, so raising the
  user-settable `scroll` config (`src/config.rs`) can never reintroduce this crash even if
  `config.rs`'s own bound-check is ever bypassed or a stale large value is loaded from flash at boot.
  `bytes_per_line()`'s budget counts `size_of::<ScreenLine>()` per slot (not just the two inner
  `chars`/`attrs` allocations) to cover the outer `Vec<ScreenLine>` containers (`lines` and
  `scrollback`) themselves; this only holds because `Default::default()` pre-reserves
  `scrollback`'s capacity to `max_scrollback + 1` up front (`scroll_up`'s push-then-`remove(0)`
  peak) instead of growing it via `Vec::new()` + amortized doubling, which would let its real
  backing capacity overshoot the budgeted count. Any future change to how `scrollback` grows must
  preserve that fixed pre-reserved capacity or the container-overhead accounting goes stale again.
  Re-run the two tests above (and re-derive the budget) if `HEAP_SIZE` (`src/heap.rs`) or the screen
  geometry (font/`SCREEN_WIDTH`/`SCREEN_HEIGHT`) ever changes.
- `keygen save [force]` / `keygen load [force]` (`src/sshkey.rs`) export and restore the private key
  as `ssh_key.hex` in the SD card root, reusing `src/storage.rs`'s `STORAGE`/`VolumeManager` (same
  SPI0 pins as `ls`) rather than a second filesystem stack; the push-to-talk I2S mic is on
  `PIN_2`/`PIN_3`/`PIN_21` and is untouched by this. Restoring is deliberately explicit-only: there
  is NO boot-time auto-restore, because silently adopting a key from whatever card happens to be
  inserted would hand that card the device's identity without an announced action. Keep it that
  way; if a boot restore is ever added it must be announced on the console and must only apply when
  no key is stored at all. The file holds the same 64-char hex the config store keeps, with its
  parser in `terminal-model/src/keyfile.rs` so the edge cases (trailing newline, wrong length,
  non-hex) stay host-tested.
- On a NixOS-style agent sandbox where plain `cargo`/`rustc` aren't on `PATH`, the working
  toolchain lives under `$RUSTUP_HOME/toolchains/nightly-x86_64-unknown-linux-gnu/bin` (set
  `RUSTUP_HOME=/home/ai/.rustup` and prepend that dir to `PATH`); building anything host-targeted
  (build scripts, proc-macros, or the `terminal-model` host tests) also needs a C linker, which
  isn't present by default — `nix-shell -p gcc --run '<cargo command>'` supplies one. `cargo check
  --features <chip>` on the root package additionally needs the `embassy` git submodule checked
  out (`git submodule update --init embassy`) because `src/net.rs` embeds cyw43 firmware blobs
  from it via `include_bytes!`; `pico-sdk`/`picotool` are unrelated C build tooling and don't need
  to be initialized for a Rust-only check/build.

- The `embassy/` git submodule is reference material only, NOT what actually gets compiled: every
  `embassy-*` line in `Cargo.toml` is a bare `version = "*"` with no `path`/`git` override, so
  Cargo resolves them from crates.io (check `Cargo.lock` — e.g. `embassy-rp` resolves to a released
  `0.4.0`, which can be well behind the submodule's pinned commit). The two can have materially
  different APIs (e.g. `0.4.0` uses the older `embassy_rp::{Peripheral, PeripheralRef, into_ref!}`
  peripheral-ownership style throughout its `pio` module, while the submodule's HEAD has moved to a
  newer `Peri<'d, T>` style) — always check the actual installed crate source
  (`~/.cargo/registry/src/*/embassy-rp-<version>/`, fetch it with `cargo fetch` first if absent)
  before writing code against any embassy-rp API, rather than trusting the submodule's source.
  `src/psram.rs` is the up-to-date, actually-building example of this project's real PIO/DMA idiom
  (`PeripheralRef`, `into_ref!`/`PeripheralRef::new`, `pio_asm!` via
  `embassy_rp::pio::program::pio_asm`) to copy from.
- Push-to-talk voice capture (`src/mic.rs`) captures mic audio on a held button and streams it to a
  network host; the receiving/transcribing side is a separate, not-yet-built process outside this
  repo. It claims PIO2 (unclaimed elsewhere — PIO0 is WiFi, PIO1 is PSRAM) for a hand-written I2S RX
  PIO program (embassy-rp ships no I2S RX driver, only the TX-only `pio_programs::i2s`; `mic.rs`'s
  program is the mirror image of that driver's `pio_asm!` block, `in pins, 1` instead of
  `out pins, 1`, unverified on real hardware) and the pins freed by removing SD card support (see
  below): `GP16`/`GP17`/`GP18` = I2S `BCLK`/`WS`/`SD` (`GP19`/`GP22` spare). Capture is 16 kHz/16-bit
  mono in ~25ms chunks, buffered through an `embassy_sync::channel::Channel` whose `Box<[i16; _]>`
  payloads land in the `DualHeap`'s PSRAM tier under primary-heap pressure (see the heap-budget
  entry above) — deliberately not a lock-free structure, per `heap.rs`'s CAS-vs-PSRAM `FIXME`.
  Button binding is plain `Key::F1`. Arming and stopping are independently
  gated: arming requires `KeyState::Pressed` with `Modifiers::NONE` (so Ctrl+F1 still reaches the
  existing reboot shortcut), while stopping fires on `KeyState::Released` whenever `mic::is_recording()`
  (reusing `mic.rs`'s `RECORDING` flag) is set, with no modifier re-check, so a release always ends
  the recording even if a modifier went down mid-hold. The decision table itself lives in
  `terminal-model/src/key_dispatch.rs`'s `ptt_action` (host-tested with
  `cargo test -p terminal-model --target x86_64-unknown-linux-gnu key_dispatch`) and `src/keyboard.rs`
  is only the I2C/`KeyReport`-to-`ptt_action` adapter, so rebinding means changing the single
  `Key::F1` check passed to `ptt_action` there and updating that module's tests.
  `capture_task` also self-stops after `MAX_RECORDING_DURATION` (60s) in case the keyboard
  link drops the `Released` report entirely. `Key::ButtonLeft2`, tried first, turned out to correspond to no physical control
  on real hardware - the PicoCalc has one D-pad and no joystick, and `ButtonLeft2` belongs to a
  `Joy*`/`Button*` group of raw keyboard-protocol codes (`src/keyboard.rs`'s `Key` enum and its
  `From<u8>` impl) that looks like it comes from a joystick/gamepad-bearing variant of this same
  keyboard co-processor protocol, not this device - treat that whole code group as suspect for any
  future key binding on this hardware. Destination is `config set ptt_host`/`config set ptt_port` (plain
  `sequential_storage` keys, no special-casing needed in `config.rs`). Wire format (needed by
  anything implementing the receiving side): one TCP connection per utterance, opened on button
  press and closed on release; each frame is a 4-byte little-endian `u32` byte count followed by
  that many bytes of raw signed 16-bit little-endian mono PCM. No handshake, no other framing.
- SD card support (`storage.rs`, the `ls` command, `README-DEVICE.md`'s old "TF Card reader"
  section) was removed to free `GP16`/`GP17`/`GP18`/`GP19`/`GP22` for the mic above — it was an
  undocumented, listing-only (no file content, no README.md feature mention), SPI0-only local
  command with no interaction with the SSH/terminal workflow this project exists for. If it's ever
  needed again, `git log` for its removal commit has the full original implementation to revert.

## Maintaining this file

Keep this file for knowledge useful to almost every future agent session in this project.
Do not repeat what the codebase already shows; point to the authoritative file or command instead.
Prefer rewriting or pruning existing entries over appending new ones.
When updating this file, preserve this bar for all agents and keep entries concise.
