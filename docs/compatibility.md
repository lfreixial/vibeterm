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
| Truecolor / 256 colors / inverse / underline | Tested | Fixture and engine assertions |
| Wrapped/wide selection text | Engine-tested | Exact string assertion |
| Physical selection / copy / primary / paste | Not verified | KWin prevented synthetic pointer targeting |
| Legacy / SGR mouse | Encoding-tested | Real TUI mouse interaction pending |
| Application cursor keys | Encoding-tested | Desktop TUI keys also work |
| Application keypad / Kitty keyboard | Not implemented | Kitty mode disabled |
| Bracketed paste envelope | Encoding-tested | ESC filtering and line endings asserted |
| Focus reporting | Implemented, not manually tested | Bypasses history/selection changes |
| IME | Implemented, not tested with real IME | Preedit/commit and candidate positioning |
| Fractional / mixed-monitor scaling | Not tested | Layout recomputes from scale factor |
| Config validation / rejected reload | Tested on X11 | Visible rejection, live shell survives, valid reload succeeds |
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
5. Load a temporary config, make its font size/color invalid, and Ctrl+Shift+R.
   The old settings/session should survive with an error. Correct it and reload.
6. Compose/commit using a real IME, checking for duplicate input. Move between
   monitors with differing fractional scales and resize.
7. Minimize 30 seconds with blink off, restore, and check output/CPU. Close with a
   foreground job running and verify the documented lifecycle.
8. Repeat relevant checks separately on native Wayland and X11.

Compilation alone is never a passed visual, clipboard, input, or hardware test.
