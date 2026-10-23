# Architecture

`session.rs` owns the engine adapter and PTY. Portable PTY creates the child and
master; one nonblocking Linux reactor performs all reads, ordered partial writes,
and resize ioctls. It polls the PTY and a wake socket, sleeping indefinitely when idle.
There is no competing engine event loop or second PTY reader. A waiter observes
exit with `waitid(WNOWAIT)`; the reactor retains PID ownership until the final reap,
so shutdown cannot signal a reused shell PID.

Alacritty's fair mutex protects terminal state. Each read parses at most 16 KiB
under the lock. The renderer snapshots visible cells, releases the lock, then does
font/GPU work. It never clones history per frame. Output bytes are never discarded;
only redundant UI notifications are coalesced. Input has 32 bounded queue entries,
256 KiB maximum per request, and one partially written buffer. Full user-input
queues reject the whole new request with an error; full engine-reply queues stop
the session explicitly. Clipboard requests have a separate eight-entry queue and
worker retaining ownership. At most one config reload read runs at a time.

Close sends SIGHUP to the foreground job group and shell, gives the shell 200 ms,
then uses SIGKILL if necessary and reaps it. Joining happens during final teardown.
Intentionally daemonized/disowned jobs are outside this policy. There is no session
supervisor, persistence, or guaranteed graceful shutdown after a GUI-process crash.

`render.rs` uses wgpu for cell rectangles and Glyphon for cached text in one surface.
Cosmic Text supplies system-font discovery, fallback, shaping, and rasterization.
Shaped cell buffers have a 4,096-entry LRU; CPU raster cache resets above 8,192
entries; the GPU atlas has library/device limits. Geometry caps at 500×200 cells,
history defaults to 10,000 lines and caps at 100,000. These are count bounds, not
a universal RAM budget. Wide cells occupy two columns; combining marks shape with
their base. Cross-cell ligatures/full bidi layout are not implemented.

`app.rs` owns native events, focus, IME, selection, clipboard requests, and reload.
Idle uses `ControlFlow::Wait`; optional blinking schedules 600 ms wakeups only while
focused/visible. Redundant resize/title/IME-position updates are suppressed. The
first desktop run exposed a configure/redraw feedback loop; this was fixed before
the baseline. Lost surfaces reconfigure, temporary errors retry after 100 ms, and
zero-size windows skip rendering. Fatal GPU errors return diagnostics and tear down
the session. Rendering currently redraws the full visible grid on damage.

## Dependencies and rationale

Rust 1.90.0 is pinned. Versions/licenses below were checked against the actual
downloaded manifests and upstream APIs.

| Dependency | Version | License |
| --- | --- | --- |
| alacritty_terminal | 0.26.0 | Apache-2.0 |
| portable-pty | 0.9.0 | MIT |
| winit | 0.30.13 | Apache-2.0 |
| wgpu | 25.0.2 | MIT / Apache-2.0 |
| glyphon | 0.9.0 | MIT / Apache-2.0 / Zlib |
| cosmic-text | 0.14.2 | MIT / Apache-2.0 |
| arboard | 3.7.0 | MIT / Apache-2.0 |

Glyphon 0.9/wgpu 25 were chosen as a compatible pair, rather than mixing unrelated
latest majors. This combination compiled and rendered on the actual desktop.
Glyphon avoids a bespoke glyph atlas. The mature Alacritty parser stays behind a
session boundary; portable-pty supplies process creation and a single ownership model.
No browser renderer, bespoke parser, telemetry, plugin host, or AI scaffolding exists.

## Protocol and security boundaries

The child gets `TERM=xterm-256color`, `COLORTERM=truecolor`, `TERM_PROGRAM=vibeterm`.
System xterm terminfo is the legacy compatibility target, not a promise of all xterm
extensions. Kitty keyboard and OSC 52 are explicitly disabled. Graphics protocols,
OSC 7/133 metadata, hyperlink activation, and application keypad encoding are absent.
Color/pixel-size query events are not serviced yet; engine capability reports need
a broader audit before daily-driver claims. Titles are bounded and control-filtered.
PTY data, clipboard text, and environment values are not logged by the application.
Config follows XDG; no user log files, runtime sockets, or saved state are created.

Upstream: [engine](https://docs.rs/alacritty_terminal/0.26.0/alacritty_terminal/),
[winit](https://docs.rs/winit/0.30.13/winit/),
[Glyphon dependency manifest](https://docs.rs/crate/glyphon/0.9.0/source/Cargo.toml),
[portable-pty](https://docs.rs/portable-pty/0.9.0/portable_pty/).
