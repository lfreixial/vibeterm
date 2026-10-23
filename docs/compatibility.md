# Compatibility and manual acceptance

Environment: CachyOS, KWin Wayland/XWayland, Radeon RX 9070 XT, RADV/Mesa 26.2.4,
Rust 1.90.0. Engine tests, real PTYs, and desktop interaction are distinguished below.

| Area | Result | Evidence or remaining check |
| --- | --- | --- |
| Wayland native window / Vulkan | Tested | Real desktop launch |
| X11/XWayland / Vulkan | Tested | Targeted script and screenshots |
| OpenGL fallback | Not tested | Actual adapter used Vulkan |
| Bash | Tested | Interactive command entry, resize, exit |
| Zsh / Fish | Not tested | Installed; interactive acceptance pending |
| Neovim / tmux | Not tested | Not installed in inspected environment |
| less | Tested on X11 | Alternate screen, PageDown, search, return to Bash |
| btop | Not tested | Installed; interaction pending |
| PTY input/output/resize/reap | Tested | Real subprocess suite |
| Split UTF-8 / combining / wide cells | Tested | Replay assertions, visible fixture |
| Basic emoji / fallback / bold / italic | Visually tested | Complex clusters need more coverage |
| Status bar at maximum font/spacing | Tested | Independent UI metrics; [72 px / 3× spacing capture](footer-maximum-zoom.png), plus layout checks at 1×/1.25×/2× scales |
| Truecolor / 256 colors / inverse / underline | Tested | Fixture and engine assertions |
| Wrapped/wide selection text | Engine-tested | Exact string assertion |
| Physical selection / copy / middle-click primary paste | User-verified | Owner reports these work (2026-10-23); display backend not specified |
| Clipboard paste shortcut / cross-backend mouse coverage | Not fully verified | KWin prevented synthetic pointer targeting; broader manual coverage pending |
| Legacy / SGR mouse | Encoding-tested | Real TUI mouse interaction pending |
| Application cursor keys | Encoding-tested | Desktop TUI keys also work |
| Application keypad / Kitty keyboard | Not implemented | Kitty mode disabled |
| Bracketed paste envelope | Encoding-tested | ESC filtering and line endings asserted |
| Focus reporting | Implemented, not manually tested | Bypasses history/selection changes |
| IME | Implemented, not tested with real IME | Preedit/commit and candidate positioning |
| Fractional / mixed-monitor scaling | Partial | Native launch at scale 1.5 succeeds; monitor transitions and visual/input acceptance remain pending |
| Config validation / rejected reload | Tested on X11 | Visible rejection, live shell survives, valid reload succeeds |
| Tabs / both split directions / focus / closure | Tested on X11 | Targeted independent-shell markers, hidden exit, screenshots |
| History search / selected-match copy | Tested on X11 | Literal history match copied through real clipboard |
| Command palette / remapped shortcuts | Tested on X11 | Palette action and live Alt+T override |
| Automatic config watching / atomic replacement | Tested | Watcher tests and desktop PTY geometry checks |
| Idle redraw | Tested | Feedback loop fixed; baseline recorded separately |
| Minimize / surface loss | Handled, not fault-injection tested | Zero-size skip and surface retry |
| Native accessibility | Not implemented | No screen-reader accessibility tree |

## Manual checklist

1. Launch the release binary, type commands, use Ctrl+C/Ctrl+D, resize rapidly,
   and exercise `less`, `vim`/`nvim`, `tmux`, and `btop` through clean exit.
2. Run `python3 scripts/visual-fixture.py`. Check combining accents, CJK, emoji,
   box drawing, styling, selection across wide cells, and copied wrapped lines.
3. Run `seq 1 1000`; scroll, drag a selection, copy into another app and paste back,
   and test middle-click primary. Hold Shift in a mouse-aware TUI. Drag-autoscroll
   is not implemented yet.
4. Paste multiline text in Bash/Zsh/Fish and inspect it before Enter. Test real
   keyboard modifier combinations and key repeat.
5. Load a temporary config, make its font size/color invalid, and save it.
   The old settings/session should survive with an error. Correct it and save again.
   Also test explicit Ctrl+Shift+R reload.
6. Compose/commit using a real IME, checking for duplicate input. Move between
   monitors with differing fractional scales and resize.
7. Minimize 30 seconds with blink off, restore, and check output/CPU. Close with a
   foreground job running and verify the documented lifecycle.
8. Repeat relevant checks separately on native Wayland and X11.

Compilation alone is never a passed visual, clipboard, input, or hardware test.

Status-bar regression verification (2026-10-23): library tests (5), clippy, and the
release build passed. An isolated X11 window at 72 px font / 3× line spacing was
captured and visually inspected; the status text stays inside its bar. The broader
X11 script did not complete in this desktop session (resize-step timeout), so this
follow-up is a focused rendering check, not a fresh pass of the full desktop suite.

Workspace follow-up (2026-10-23): the complete X11 script, including resize and
`--workspace` acceptance checks, passed. The resize timeout above did not recur;
this does not establish its original cause. See [workspace capture](workspace.png),
[palette](palette.png) and [history search](search.png). Native Wayland IME and
mixed-monitor interaction remain separate manual acceptance items.
