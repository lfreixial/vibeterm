# Bash and Zsh shell integration

New panes running the default/configured `bash` or `zsh` install temporary shell
hooks automatically. No startup files in your home directory are changed. Other
shells and explicit CLI `-e` commands run unchanged. Disable automatic installation
with `shell_integration = false`; this takes effect for new panes.

| Action | Default shortcut |
| --- | --- |
| Previous command | Ctrl+Shift+Up |
| Next command / return to bottom | Ctrl+Shift+Down |
| Copy last command output | Ctrl+Shift+Y |

The command palette exposes the same actions. Override `previous_command`,
`next_command`, or `copy_last_output` in `[keybindings]` as usual. Commands already
visible near the bottom may not move the viewport; repeated Up continues to older
commands. These actions operate on the focused pane.

Copy captures the terminal's displayed output at completion, including stdout and
stderr, with styling removed and soft-wrapped lines joined. It excludes the echoed
command and prompt. While a command runs, the previous completed output is still
available. Empty output copies an empty string. A command completes when the shell
returns to its prompt; a command that exits/replaces the shell has no completion hook.

## Limits and shell compatibility

- Up to 1,024 command positions are retained, and positions evicted from scrollback
  are discarded. No command log or output is written to disk.
- The latest completed output is retained in memory, capped at 1 MiB. Extraction
  also has a conservative 262,144-cell limit. Output whose beginning has already
  left scrollback is unavailable, rather than silently copied in part.
- Resizing/reflow, clearing the screen/history, and structural screen edits reset
  position tracking. A completed output copy survives these operations. Resizing
  during a command makes that command's output unavailable. Fullscreen applications
  are not captured; this feature is intended for ordinary line-oriented commands.
- Bash reads `.bashrc` and preserves scalar/array `PROMPT_COMMAND`. If an existing
  DEBUG trap or function tracing is configured, automatic hooks are skipped to
  preserve the user's setup. Bash frameworks that install their own DEBUG trap
  later can also disable tracking. Existing shell-specific shortcuts are unaffected
  except for the three terminal shortcuts listed above.
- Zsh reads `.zshenv` and `.zshrc` through a temporary `ZDOTDIR`, restoring the user's
  directory (including changes from `.zshenv`). Existing prompt/preexec hooks are
  retained. A nonprinting marker is added to `PROMPT_EOL_MARK` so Zsh's synthetic
  missing-newline indicator is not copied. Startup files that disable RCS or replace
  the hook arrays/marker later can disable tracking. Custom prompt frameworks need
  broader manual acceptance testing.
- Nested shells, remote SSH shells, and tmux do not automatically receive hooks.

## Implementation and verification

Hooks emit private `OSC 2 ; __vibeterm_shell__; A/C/D` markers for prompt start,
command execution, and completion. The terminal consumes them instead of changing
the title. This uses the existing ANSI parser and its synchronized-update buffering;
it does not run a second parser, scan scrollback every frame, or poll idle shells.
This private transport is not general OSC 133 support.

A forwarding ANSI handler observes scrolling in constant time. It retains absolute
row coordinates rather than updating every command position for every line of output.
Output extraction happens once at command completion. Geometry changes conservatively
invalidate coordinates rather than guessing after reflow.

Tests cover actual Bash/Zsh startup, existing hooks and exit status, missing final
newlines, wrapped Unicode/styled output, synchronized updates, bounded scrollback,
resize invalidation, fullscreen output rejection, and equivalence with the original
terminal handler. Desktop shortcut/clipboard acceptance is recorded separately in
[compatibility](compatibility.md).

Local release replay comparison (2026-11-13, Rust 1.99, three alternating runs per
mode, one million lines, 110×32, 10,000 history rows):

| Synthetic input | Bare handler median | Tracked handler median |
| --- | ---: | ---: |
| Plain ASCII | 142.59 MiB/s | 134.00 MiB/s |
| Styled / Unicode | 150.86 MiB/s | 140.78 MiB/s |

This is about 6–7% lower **parser-only** throughput, not an end-to-end rendering
measurement. The fixture excludes command completion/extraction. Reproduce with
`cargo run --release --locked --example replay -- 1000000 plain bare` and replace
`bare` with `tracked`, or `plain` with `mixed`. Disabling `shell_integration` for new
panes bypasses the tracking handler and its mutex entirely.

The X11 smoke test (`scripts/x11-smoke.py target/release/vibeterm --shell`) verified
automatic Bash setup, exact clipboard text through both Ctrl+Shift+Y and the palette,
navigation back to a responsive shell, and clean exit. The navigation screenshot
was inspected: the selected command and its first output line were at the viewport
start. Real Zsh was verified through PTY tests; custom prompt frameworks and native
Wayland desktop shortcuts still need manual acceptance.
