"""Workspace acceptance checks, called by x11-smoke.py --workspace."""
from pathlib import Path
import subprocess
import time


def run(work, config, key, type_text, wait_for, capture, binary, env):
    shell = work / "test-shell"
    shell.write_text('#!/bin/sh\nprintf "%s\\n" "$$" >> shells.txt\nexec /bin/bash --noprofile --norc\n')
    shell.chmod(0o755)
    base = f'font_size = 15\nshell = "{shell}"\n'
    config.write_text(base)
    time.sleep(0.4)

    def expect(name, value):
        path = work / name
        wait_for(lambda: path.exists() and path.read_text() == value,
                 f"{name}: expected {value!r}, got {path.read_text() if path.exists() else 'no file'}")

    def check_pane(name, value):
        type_text(f'printf "%s" "$PANE" > {name}\n')
        expect(name, value)

    def spawn(symbol, count, modifiers=5):
        key(symbol, modifiers)
        wait_for(lambda: (work / "shells.txt").exists()
                 and len((work / "shells.txt").read_text().splitlines()) == count,
                 "New pane shell did not start")
        time.sleep(0.25)

    type_text('PANE=one\n')
    spawn(ord('E'), 1)
    type_text('PANE=two\n')
    check_pane('two.txt', 'two')
    key(0xff51, 5)  # Previous pane
    check_pane('one.txt', 'one')
    spawn(ord('O'), 2)
    type_text('PANE=three\n')
    check_pane('three.txt', 'three')
    capture('splits.png')

    def grid(name):
        type_text(f'stty size > {name}\n')
        path = work / name
        wait_for(lambda: path.exists() and len(path.read_text().split()) == 2,
                 f"{name}: no stty output")
        return tuple(map(int, path.read_text().split()))

    # Pane order is one, three, two; three is focused.
    three = grid('three-size.txt')
    key(0xff53, 5)
    two = grid('two-size.txt')
    key(0xff51, 5)
    for _ in range(4):
        key(ord('='), 5)  # Ctrl+Shift+= zooms the focused pane only
    time.sleep(0.3)
    zoomed = grid('three-zoomed.txt')
    if not (zoomed[1] < three[1] and zoomed[0] <= three[0]):
        raise SystemExit(f"Pane zoom did not shrink the grid: {three} -> {zoomed}")
    capture('pane-zoom.png')
    key(0xff53, 5)
    if grid('two-after-zoom.txt') != two:
        raise SystemExit("Pane zoom changed a neighbouring pane")
    key(0xff51, 5)
    key(ord('0'), 5)  # Ctrl+Shift+0 restores the window font
    time.sleep(0.3)
    if grid('three-reset.txt') != three:
        raise SystemExit("Pane zoom reset did not restore the grid")
    print('PASS: per-pane zoom resizes only the focused pane and resets', flush=True)

    spawn(ord('T'), 3)
    type_text('PANE=four\n')
    check_pane('four.txt', 'four')
    key(0xff09, 5)  # Previous tab
    check_pane('restored-three.txt', 'three')
    capture('tabs-and-splits.png')
    key(0xff09, 4)  # Next tab
    check_pane('restored-four.txt', 'four')
    key(ord('W'), 5)
    time.sleep(0.2)
    check_pane('after-tab-close.txt', 'three')
    key(ord('W'), 5)
    time.sleep(0.2)
    check_pane('after-split-close.txt', 'one')
    key(0xff53, 5)
    check_pane('surviving-two.txt', 'two')
    key(ord('W'), 5)
    time.sleep(0.2)
    check_pane('sole-pane.txt', 'one')
    print('PASS: independent shells, both split directions, tab/pane focus restoration and close', flush=True)

    # Needle output is assembled by Bash so it cannot match the command echo.
    type_text("printf 'history-%s\\r\\n' '[needle].'; seq 1 250; printf done > history.txt\n")
    expect('history.txt', 'done')
    time.sleep(0.1)
    key(ord('F'), 5)
    type_text('history-[needle].')
    time.sleep(0.5)
    key(0xff0d)  # Search again after a burst of key events if the worker was busy.
    time.sleep(0.2)
    capture('search.png')
    key(0xff1b)  # Escape must be consumed by search.
    key(ord('C'), 5)
    time.sleep(0.2)
    helper = Path(binary).parent / 'examples/clipboard_probe'
    assert helper.exists(), 'Build --examples before workspace acceptance'
    copied = subprocess.check_output([str(helper)], env=env, text=True).strip()
    assert copied == 'history-[needle].', repr(copied)
    type_text('printf search-closed > search-closed.txt\n')
    expect('search-closed.txt', 'search-closed')
    print('PASS: literal scrollback search, selected match copied through real clipboard, Escape returns to shell', flush=True)

    key(ord('P'), 5)
    capture('palette.png')
    type_text('New tab')
    spawn(0xff0d, 4, 0)
    type_text('PANE=palette\n')
    check_pane('palette.txt', 'palette')
    key(ord('W'), 5)
    time.sleep(0.2)
    check_pane('palette-return.txt', 'one')
    print('PASS: filtered command palette executes a workspace action', flush=True)

    def size(name):
        type_text(f'stty size > {name}\n')
        path = work / name
        wait_for(lambda: path.exists() and len(path.read_text().split()) == 2, 'PTY size missing')
        return tuple(map(int, path.read_text().split()))

    before = size('before-reload.txt')
    config.write_text(base.replace('15', '22', 1))
    time.sleep(0.4)
    after = size('after-reload.txt')
    assert after[0] < before[0] and after[1] < before[1], (before, after)
    config.write_text('font_size = 0\n')
    time.sleep(0.4)
    assert size('invalid-reload.txt') == after
    capture('automatic-reload-error.png')
    replacement = config.with_suffix('.tmp')
    replacement.write_text(base + '\n[keybindings]\nnew_tab = "Alt+t"\n')
    replacement.replace(config)
    time.sleep(0.4)
    assert size('atomic-reload.txt') == before
    spawn(ord('t'), 5, 8)
    type_text('PANE=remapped\n')
    check_pane('remapped.txt', 'remapped')
    key(ord('W'), 5)
    time.sleep(0.2)
    check_pane('final.txt', 'one')
    config.write_text(base)
    time.sleep(0.3)
    # Exiting a hidden pane must remove the tab even while its redraw is coalesced.
    spawn(ord('T'), 6)
    type_text('sleep 1; exit\n')
    key(0xff09, 5)
    time.sleep(1.4)
    check_pane('hidden-exit.txt', 'one')
    key(0xff09, 4)
    check_pane('hidden-tab-removed.txt', 'one')
    print('PASS: automatic/atomic reload, invalid config recovery, custom shortcut and hidden child exit', flush=True)
