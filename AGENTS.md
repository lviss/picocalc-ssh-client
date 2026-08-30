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

## Maintaining this file

Keep this file for knowledge useful to almost every future agent session in this project.
Do not repeat what the codebase already shows; point to the authoritative file or command instead.
Prefer rewriting or pruning existing entries over appending new ones.
When updating this file, preserve this bar for all agents and keep entries concise.
