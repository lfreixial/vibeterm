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

## Workspace release follow-up — 2026-10-23

The M2 workspace build passes 23 tests, fmt, clippy and release compilation.
The release X11 script with `--workspace` passes all keyboard/TUI/workspace checks,
including real clipboard retrieval of a selected search match. Optional pointer
checks remain separate. All six spawned workspace test shells were reaped.

Native Wayland, same machine, blink disabled:

| Measurement | Observation | Scope |
| --- | --- | --- |
| Idle CPU | 0.000% of one core | Four-second `/proc` sample; timer-resolution limit still applies |
| Idle RSS | 86,784 KiB (~84.8 MiB) | Main process, nearly empty history |
| Idle frames | 3 | Seven-second fixture, including startup/output |
| Warm first frame | 74.49, 81.26, 76.87 ms | First submission, not shell readiness |
| Million-line fixture | 0.810 s | Whole process wall time including startup/exit |
| Output frame work | p50 0.831 ms / p95 0.964 ms | 150 frames; CPU + acquisition + submission |

The idle window was 1060×700 at scale 1. The compositor placed the subsequent
warm-start/output windows at 1590×1050, scale 1.5, grid 110×32. These are not
identical conditions to the previous baseline and do not establish a speedup.
Opening at scale 1.5 also does not replace mixed-monitor interaction testing.

The release X11 workspace scenario presented 1,043 frames, p50 5.869 ms / p95
6.069 ms including acquisition/submission. This still exceeds a 240 Hz interval
and does not demonstrate 240 Hz interaction. Raw data is in
[workspace-release-check.txt](workspace-release-check.txt).
Multi-pane sustained-load and physical input-latency measurements remain pending.

## Comparison with other terminals — 2026-10-25

`python3 scripts/compare.py target/release/vibeterm 5` runs identical workloads in each
installed terminal with default settings, a 110×32 grid (Konsole honoured 110×28),
native Wayland, same machine as above. Throughput is how fast the terminal consumes a
pre-built burst written to its PTY (`tcdrain` waits until it has read everything);
it is not frame rate or input latency. Startup is launch until a command runs inside
it. Medians of 5 runs (10 for startup); run-to-run spread was ≤3%.

| Measurement | vibeterm (before → after) | Alacritty 0.17 | Kitty 0.49 | Konsole 26.08 |
| --- | --- | --- | --- | --- |
| Plain text, 2M lines | 108 → **138 MiB/s** | 109 | **161** | 35 |
| Coloured Unicode, 500k lines | 107 → **134 MiB/s** | 109 | 100 | 43 |
| Full-screen redraws, 10k frames | 123 → **167 MiB/s** | 131 | **204** | 46 |
| Startup until a command runs | 69.5 → **31.2 ms** | 56.9 | 133.8 | 96.5 |
| Idle RSS | 84.3 → **65.6 MiB** | 94.8 | 124.6 | 100.1 |
| Idle CPU | 0% | 0% | 0% | 0% |

What changed, each verified by measurement before it was kept:

- Vulkan-only GPU instance, OpenGL only as fallback: −20 ms startup, −17 MiB RSS.
- Font database loaded on a thread from process start; shell spawned while the GPU
  initialises: first frame ≈70 → ≈32 ms; shell running at 31 ms.
- PTY reads pipelined with parsing on separate threads: +25–34% throughput.
  Parser-only (`examples/replay`) ceiling is ≈141 MiB/s plain / ≈152 mixed.
- Status/tab text shaping cached: draw CPU per frame p50 0.83 → 0.36 ms.
- Allocation-free cell snapshots that skip blank cells: 212 → 163 µs per snapshot
  (taken under the terminal lock, so this also shortens parser stalls).
- Swapchain frame latency 2 → 1 (not measurable here without photon hardware).

Tried and rejected: fat LTO and `panic = "abort"` (no measurable parser gain).
Kitty's remaining lead on plain text and redraws comes from its own SIMD-optimised C
parser; vibeterm now runs close to the Alacritty engine's own ceiling, so further gains
there require engine changes. PSS varies with how many other processes share the same
driver libraries, so compare it only within a single run.

## Damage-based rendering — 2026-10-25

Process CPU (user+sys, median of 5) for interactive workloads, minus the ≈70 ms that
startup, exit and the Python driver cost in every build. "Typing" echoes one character
every 16 ms (300 frames); "ticker" rewrites one status line under a screen of text.

| Workload | Rendering CPU before | After |
| --- | --- | --- |
| Typing | 170 ms (595 frames) | **80 ms (301 frames)** |
| One-line ticker | 160 ms | **80 ms** |

Snapshots re-copy and the canvas repaints only damaged rows, so unchanged panes cost
nothing. Typing previously drew two frames per keystroke: Wayland compositors answer
every IME cursor-area update with an empty preedit, which triggered a redraw; no-op
IME events are now ignored. Output floods damage every row, so they repaint fully and
add one canvas copy: plain/unicode/redraw throughput measured 136/133/162 MiB/s,
within ≈3% of the previous run.


## Interaction follow-up — 2026-11-13

Baseline: v0.1.1, X11/XWayland (now the default when DISPLAY exists), same local
AMD GPU/240 Hz desktop. These measurements are not directly comparable with the
older native-Wayland comparison above. Desktop runs were sequential, with other
applications running; treat single-run throughput/startup values as observations.

`examples/interaction_probe.rs` provides a repeatable **headless PTY** probe:

```sh
cargo run --release --locked --example interaction_probe
```

A burst of 5,000 resize requests while the terminal lock is held models a busy
renderer/parser. The old 32-entry input queue rejected 4,967 requests in all three
runs and failed to reach the final requested width before the five-second deadline.
The new separate resize slot accepted all requests and reached the final width in
all three runs (0.131–0.151 ms from enqueue start through the post-release check).
This controlled empty-history test establishes queue correctness, not real-world
drag latency or reflow cost with a full history. Regression tests also verify the
kernel PTY size and that queued keyboard input still reaches the child.

| Headless PTY measurement, three runs | Before | After |
| --- | --- | --- |
| Rejected requests per 5,000-resize burst | 4,967 | 0 |
| Final requested width reached | No | Yes |
| Input-to-title-ack p95, no background output | 0.159 ms | 0.156–0.158 ms |
| Input-to-title-ack p95, three busy background sessions | 0.165–0.311 ms | 0.167–0.311 ms |

The acknowledgement probe polls every 100 microseconds and does not render a
window. Its polling/scheduling floor dominates these tiny values; there is no
established typing-latency improvement. Raw runs:
[before](interaction-before.txt), [after](interaction-after.txt).

`--diagnostics` now splits renderer frame wall time into CPU preparation/submission,
surface acquisition, and the presentation call. On one million-line desktop run
with MangoHud disabled, the updated build measured:

| Component | p50 | p95 |
| --- | --- | --- |
| Renderer total | 5.892 ms | 6.106 ms |
| CPU preparation/submission | 0.270 ms | 0.337 ms |
| Surface acquisition wait | 5.614 ms | 5.829 ms |
| Presentation call | 0.011 ms | 0.014 ms |

These are CPU wall-clock intervals inside the renderer, not GPU execution,
terminal snapshot/lock time, or physical key-to-photon latency. Component
percentiles need not add up. The major wait in this run is surface acquisition;
no presentation-mode or frame-latency tradeoff was changed.

A separate baseline check found the inherited MangoHud overlay affects idle CPU
and startup: a four-second idle sample reported 2% of one core with it enabled,
and 0% with it disabled (both essentially idle rendering). Three subsequent warm
first-frame observations were 65–67 ms with it enabled and 32–36 ms disabled.
These are local sequential samples, not a universal speedup claim. The application
does not override the user's overlay settings. For an overlay-free baseline:

```sh
DISABLE_MANGOHUD=1 python3 scripts/measure.py target/release/vibeterm
```

Million-line process wall time without the overlay was 0.565 s before and 0.580 s
after, a single-run difference that does not establish a throughput change.
Real pointer-drag latency, GPU execution timings and multi-monitor interaction
still require desktop/manual measurement.
