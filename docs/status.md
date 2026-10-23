# Milestone status

## M0 — implemented and validated

Empty repository inspected; no applicable AGENTS.md found. Native Rust application,
pinned toolchain/lockfile, and check workflow created. Build succeeded and real Vulkan
windows ran on CachyOS/KWin Wayland and X11/XWayland. Architecture, tests, configuration,
launch instructions, benchmark fixtures, and compatibility checklist are present.

## M1 — implemented; acceptance remains partial

Real PTY, ordered I/O, mature terminal parser, GPU text/backgrounds, keyboard input,
resize, bounded history, selection, clipboard, and primary selection are implemented.
Unicode, ANSI/truecolor, alternate screen, legacy mouse/focus protocols, and
bracketed paste are wired into the application.

The automated suite covers output/input, resize, child exit/reaping, paste limits,
synchronized-update timeout, split UTF-8, combining/wide cells, wrapped selection,
history bounds, modes, scroll regions, erasure, and malformed streams. Actual Bash
typing/resize and `less` paging/search/exit pass on X11; GPU text was visually inspected.
Invalid config reload was visually verified to retain the live shell; valid reload
then succeeded. The final release smoke script exits cleanly.

Owner feedback (2026-10-23) confirms selection/copy and middle-click paste work;
the display backend was not specified.

**Acceptance still required:** remaining clipboard and mouse coverage on both backends,
Wayland keyboard/TUI interaction, real IME composition, mixed-monitor scaling,
application keypad support, and a fuller terminal capability audit. KWin blocked the
test driver's pointer targeting, so those interactions were not marked passed.

## M2 — next, not complete

Existing foundations: fallback fonts, IME plumbing, font zoom, safe manual reload,
status/error footer, and Linux package files.

Remaining: tabs/splits and focus routing, scrollback search, searchable command
palette, automatic config watching, configurable shortcuts, opacity, cursor/focus
polish, selection autoscroll, broader text/input testing, performance budgets,
package installation validation, and native accessibility. Finish M1 acceptance
before expanding the feature surface.

## M3 / M4 — not started

M3: opt-in OSC 133/7 integration, command navigation/output copying, completion
status/duration/notifications, and saved layouts/directories without stored-task execution.

M4: versioned Unix-socket external-process plugins, bounded RPC, capabilities, peer
credentials, lifecycle/status controls, and a real example. No speculative host or
protocol was added before this milestone.

Backlog: AI, persistent sessions, advanced graphics protocols, remote-context work,
WASM, marketplaces/cloud, macOS/Windows, and advanced effects.
