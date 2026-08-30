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

## Maintaining this file

Keep this file for knowledge useful to almost every future agent session in this project.
Do not repeat what the codebase already shows; point to the authoritative file or command instead.
Prefer rewriting or pruning existing entries over appending new ones.
When updating this file, preserve this bar for all agents and keep entries concise.
