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

## M2 — workspace features implemented; acceptance remains partial

Tabs and nested resizable splits run independent shells, with keyboard/mouse focus,
per-tab focus restoration, pane closure and hidden-child exit handling. Limits:
eight tabs, 32 panes, minimum dimensions for creating a split. Session startup and
pane teardown run off the UI thread. Tabs are not saved across application restarts.

Literal smart-case scrollback search runs on a cancellable worker in bounded line
ranges; output revisions reject stale selections. The command palette filters
actions, shows availability and current shortcuts, and scrolls its selected row
into view in small windows. TOML shortcut overrides are validated for conflicts.
An inotify worker watches config changes and atomic replacement without idle polling;
invalid edits retain the previous settings and sessions.

The X11 workspace checks exercise independent shells, both split directions,
focus restoration, pane/tab closure, history search copied through the real clipboard,
palette execution, automatic and atomic config reload, invalid reload recovery,
custom shortcuts, and hidden-pane exit. Screenshots were visually inspected.
The earlier intermittent resize timeout did not recur in the follow-up full run;
its original cause is not established.

Follow-up (2026-10-25): split dividers can be dragged (ratios persist when panes
close), selection autoscrolls past pane edges, and new panes open in the focused
shell's `/proc` working directory. Ctrl+Space sends NUL, Control shortcuts use the
physical key on non-Latin layouts, unfocused windows show a hollow cursor, the wheel
honours the alternate-scroll mode, zoom reset returns to the configured size, and
pastes drop C0/C1 controls other than tab and newlines. Unit/PTY tests and the X11
workspace suite pass; pointer-driven divider dragging and autoscroll still need
manual acceptance because KWin blocks the pointer test driver.

Remaining: M1 backend/input acceptance above, opacity, accessibility, broader
multi-pane performance/stress checks, and package installation validation.
OSC 7 directory reporting (for remote/SSH shells, where `/proc` cannot see) belongs to M3.

## M3 / M4 — not started

M3: opt-in OSC 133/7 integration, command navigation/output copying, completion
status/duration/notifications, and saved layouts/directories without stored-task execution.

M4: versioned Unix-socket external-process plugins, bounded RPC, capabilities, peer
credentials, lifecycle/status controls, and a real example. No speculative host or
protocol was added before this milestone.

Backlog: AI, persistent sessions, advanced graphics protocols, remote-context work,
WASM, marketplaces/cloud, macOS/Windows, and advanced effects.
