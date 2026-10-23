# Initial performance evidence — 2026-10-23

These are local baseline observations, not a performance guarantee or comparison
against other terminals. No hardware-specific compiler flags were used.

## Conditions

- CachyOS; AMD Ryzen 7 5800X3D (8 cores / 16 threads).
- AMD Radeon RX 9070 XT, Vulkan through RADV/Mesa 26.2.4.
- KWin Wayland; XWayland used separately for targeted keyboard/TUI checks.
- Monitor reported 3840×2160 at 239.977 Hz in the X11 diagnostic run.
- Terminal surface 1060×700 physical pixels, scale 1, 114×32 cells.
- Font `monospace`, 15 logical pixels, 1.3 line spacing, 14 px padding.
  `fc-match monospace` resolves to Noto Sans Mono on this machine; Cosmic Text
  performs its own fontdb resolution/fallback, which was not instrumented per glyph.
- Scrollback configured to 10,000 lines. Idle sample has nearly empty history;
  output fixture fills the history. Cursor blinking disabled.
- Rust 1.90.0 release, thin LTO, one codegen unit, committed application lockfile.
- Concurrent desktop applications were running. Cold filesystem/shader caches were
  not reset. Physical key-to-photon latency and GPU wakeups were not measured.

The driver prints `radv is not a conformant Vulkan implementation, testing use only`.
Rendering succeeded; this driver warning is preserved in the diagnostic evidence.

## Final release baseline

Run `python3 scripts/measure.py` from the repository on the desktop:

| Measurement | Observation | Scope |
| --- | --- | --- |
| Idle CPU | 0.000% of one core | 4-second `/proc` main-process sample after 2-second warmup; timer resolution limits this |
| Idle RSS | 79,372 KiB (~77.5 MiB) | Nearly empty history; excludes separately counted child/GPU memory |
| Idle frames | 2 over the seven-second fixture | Startup/initial output frames; no continuous animation |
| Warm first frame | 73.06, 69.38, 69.22 ms | In-process startup to first submission; includes fonts/GPU, does not prove shell readiness |
| 1,000,000 mixed output lines | 0.802 s process wall time | Includes Python producer, PTY/parser, UI startup and shutdown |
| Output frames | 115 | Intermediate output is parsed in full; redraw requests are coalesced |
| Output frame interval work | p50 1.146 ms; p95 1.214 ms | CPU preparation + surface acquisition + submission, not GPU execution or physical latency |

The earlier baseline measured 85,716 KiB idle RSS, 71.44–75.22 ms warm first frame,
and 0.803 s for the same output load. Memory/startup vary with driver/font/cache state.
Do not treat one sample as a regression budget.

`cargo run --release --locked --example replay -- 200000` measured **15,200,000 bytes
in 0.114105 s (127.04 MiB/s)** with a 120×40 engine and 10,000-line history. This
is synthetic parser throughput only; it excludes the PTY, renderer, and desktop.

An X11 run combining scripted typing, resize, TUI interaction, and font/config reload
reported p50 5.907 ms / p95 6.180 ms over 229 frames. That includes surface waits
and setup work and exceeds a 240 Hz refresh interval (~4.167 ms). It is not evidence
that all interactive workloads meet the provisional frame-time goal. That run also
exposed a test-script readline prefix bug; the script was corrected and rerun.
The corrected final release run passed typing, PTY resize, `less`, invalid/valid
config reload, and shell exit: 218 frames, p50 5.919 ms / p95 6.148 ms. Raw diagnostics
are in [x11-release-check.txt](x11-release-check.txt); [rendering](screenshot.png)
and [reload rejection](reload-error.png) were visually inspected. The same caution
about exceeding a 240 Hz interval applies to this X11 workload.

## Checks and remaining measurements

- `cargo fmt --check`: passed.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: passed.
- `cargo test --workspace --locked`: 14 tests passed, including five real PTY tests.
- `cargo build --release --locked`: passed; runnable release binary produced.
- Example config validation and CLI help/version: passed.
- `bash -n packaging/PKGBUILD` and `desktop-file-validate`: passed.
- Full package build/install and remote CI: not run.
- Pointer/clipboard desktop automation: blocked by compositor targeting; not passed.

Pending: true cold startup, shell-ready timestamps, physical input latency under
output, GPU wakeups, scrolling/resizing distributions, multi-monitor scaling,
minimized long-duration CPU, memory over repeated create/close cycles and all cache
limits, and comparable Kitty/Ghostty/Alacritty runs. Set regression budgets only
after representative repeated measurements on a stable build/environment.
