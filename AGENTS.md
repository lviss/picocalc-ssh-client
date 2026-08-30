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
  hardware-independent terminal buffer/VTE logic (`screen_model.rs`) and vector glyph-drawing
  (`glyphs.rs`), pulled in by `src/screen.rs`. It depends only on `vte`, `embedded-graphics`, and
  `profont` — all host-buildable — so it's the place for real, runnable unit tests. Run them with
  `cargo test -p terminal-model --target x86_64-unknown-linux-gnu` (must override the default
  target set in `.cargo/config.toml`). If new logic needs a host test and doesn't fit here, prefer
  extending this crate over adding tests to the hardware-coupled root crate.
- `.github/workflows/build.yml` has two jobs: `build-pr` (on `pull_request`, builds both
  `pico2w`/`pimoroni2w` via `make CHIP=<chip> image`, uploads each as a workflow artifact named
  `picocalc-ssh-client-<chip>-pr<N>`) and `build-release`/`publish-release` (manual
  `workflow_dispatch` with a required `version` input, builds both chips, publishes one GitHub
  Release tagged `version` with both `.uf2` assets). There is no push-to-main auto-release anymore.
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
  the way through codegen; only the final link step needs `flip-link`, which may not be installed
  in every environment (`cargo install flip-link` needs network/build tools) — that's a linker
  availability gap, not a code problem, if it's the only failure.
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
- On a NixOS-style agent sandbox where plain `cargo`/`rustc` aren't on `PATH`, the working
  toolchain lives under `$RUSTUP_HOME/toolchains/nightly-x86_64-unknown-linux-gnu/bin` (set
  `RUSTUP_HOME=/home/ai/.rustup` and prepend that dir to `PATH`); building anything host-targeted
  (build scripts, proc-macros, or the `terminal-model` host tests) also needs a C linker, which
  isn't present by default — `nix-shell -p gcc --run '<cargo command>'` supplies one. `cargo check
  --features <chip>` on the root package additionally needs the `embassy` git submodule checked
  out (`git submodule update --init embassy`) because `src/net.rs` embeds cyw43 firmware blobs
  from it via `include_bytes!`; `pico-sdk`/`picotool` are unrelated C build tooling and don't need
  to be initialized for a Rust-only check/build.

## Maintaining this file

Keep this file for knowledge useful to almost every future agent session in this project.
Do not repeat what the codebase already shows; point to the authoritative file or command instead.
Prefer rewriting or pruning existing entries over appending new ones.
When updating this file, preserve this bar for all agents and keep entries concise.
