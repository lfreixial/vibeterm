# Changelog

## 0.2.0 — command navigation, links, and smoother interaction

This is a regular release, published as GitHub's Latest release.

### Added
- Automatic Bash/Zsh integration: Ctrl+Shift+Up/Down navigates commands, and
  Ctrl+Shift+Y copies the last completed command's output without its prompt or
  echoed command. These actions are also available in the command palette.
  Startup hooks use temporary files and do not edit user shell configuration.
- Ctrl+click opens explicit HTTP(S) URLs, OSC 8 hyperlinks, and local file
  references through the system default application. Configure `link_editor`
  to open file:line:column references in a chosen editor.
- Optional `link_single_click = true` opens links on click/release while dragging
  still selects text. Bare domains such as `google.com` do not open websites.
- Diagnostics separate CPU frame work from surface acquisition and presentation
  time; an interaction probe measures resize and input behavior.

### Fixed
- PTY resize bursts retain the latest geometry without filling the input queue,
  keeping input responsive while panes are being resized.

### Compatibility
- Shell integration can be disabled with `shell_integration = false` for new
  panes. Existing Bash DEBUG traps are preserved by skipping automatic hooks.
- Command positions are bounded by scrollback and reset on reflow/screen editing.
  The last completed output copy survives resizing; fullscreen application output
  and output evicted from scrollback are not captured. See
  [shell integration](https://github.com/lfreixial/vibeterm/blob/v0.2.0/docs/shell-integration.md).
- Linux only. Native Wayland file drops remain unavailable. Custom shell prompt
  frameworks, IME composition, mixed-DPI monitors, and broader compositor coverage
  still need acceptance testing; Kitty keyboard/graphics protocols and native
  accessibility remain unimplemented.


## 0.1.1 — image input and interaction fixes

### Added
- Drag image files into the terminal or paste clipboard images with Ctrl+Shift+V
  or Shift+Insert. Clipboard images are saved as private temporary PNGs and their
  paths are inserted for local tools such as Codex and Claude to read.
- Visible split dividers with hover/drag highlighting and a wider resize grab area.
- `--wayland` to explicitly select native Wayland. X11/XWayland is now preferred
  when available because the current window library supports Linux file drops
  only through that backend.

### Fixed
- Split resizing no longer reconfigures the GPU surface on every pointer movement.
  Rapid drag movements are combined into one pane resize per frame.
- Slow touchpad and high-resolution wheel scrolling preserves fractional line
  deltas instead of dropping small movements.
- Color settings reject sign characters and require exactly six hexadecimal digits.
- Background pane exits and spawn completions preserve search, the command
  palette, selection/resize drag, and IME composition when active focus is unchanged.

This remains an early preview. Native Wayland file drops are unavailable; clipboard
image files remain in the system temporary directory until user or system cleanup.
The known gaps listed for 0.1.0 still apply.

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
