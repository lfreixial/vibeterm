# Changelog

## 0.1.0 — first public preview

An early preview of a native, GPU-rendered Linux terminal. It is usable day to day
for many workflows, but has not finished acceptance testing; see *Known gaps*.

### Terminal
- Real PTY with the Alacritty terminal engine: truecolor and 256 colours, alternate
  screen, scroll regions, wide and combining characters, bracketed paste, mouse and
  focus reporting, synchronized updates.
- GPU text rendering (Vulkan, with an OpenGL fallback) with system font fallback,
  bold/italic, underline, strikeout, inverse and dim.
- Powerline separators and block elements drawn to fill the cell exactly, so tmux
  status bars and TUI graphs have no gaps.
- Selection (drag, word, line), clipboard and primary selection, middle-click paste.
- Programs such as tmux and nvim can copy to the system clipboard (OSC 52, copy only).
- Ctrl+Space, and Ctrl shortcuts on non-Latin keyboard layouts, work as expected.

### Workspace
- Tabs and nested splits with independent shells; drag a divider to resize.
- New tabs and splits open in the focused shell's current directory.
- Per-pane font zoom (Ctrl+Shift+= / − / 0) alongside window zoom (Ctrl+= / − / 0).
- Scrollback search, command palette, configurable shortcuts.
- Selection autoscrolls past a pane's edge.
- Validated TOML configuration that reloads automatically; invalid edits keep the
  previous settings and running shells.

### Performance
Measured against Alacritty 0.17, Kitty 0.49 and Konsole 26.08 on the same machine
(`scripts/compare.py`; details in `docs/performance.md`):
- Startup to a running command in about 31 ms, the fastest of the four.
- The lowest idle memory of the four (about 66 MiB RSS), with 0% idle CPU.
- Output throughput above Alacritty and Konsole, and above Kitty for coloured text.
- Only changed rows are redrawn, halving rendering CPU while typing.

### Known gaps
- Linux only. macOS is planned.
- Not yet manually verified: real IME composition, mixed-DPI multi-monitor moves,
  application keypad mode, and clipboard behaviour on every compositor.
- Kitty keyboard protocol, graphics protocols, hyperlinks and shell integration
  (OSC 7/133) are not implemented. Colour and size queries are not answered.
- No accessibility tree or screen-reader support.
- Sessions and layouts are not saved across restarts.
