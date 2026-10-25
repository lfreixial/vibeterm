#!/usr/bin/env python3
"""Runs inside a terminal: writes a pre-built workload to the PTY and records how long
the terminal took to consume it. Usage: compare_workload.py MODE OUTFILE (run by compare.py)"""
import os
import sys
import termios
import time

mode, out = sys.argv[1], sys.argv[2]
rows, cols = map(int, os.popen("stty size").read().split())


def plain():
    # 2,000,000 lines of 80-column ASCII, like `cat` of a large log.
    line = ("The quick brown fox jumps over the lazy dog 0123456789 " * 2)[:79] + "\n"
    return (line * 2_000_000).encode()


def unicode():
    # 500,000 lines of truecolor + accented + CJK + combining text.
    line = ("\033[38;2;128;203;196m0123456789 abcdefghijklmnopqrstuvwxyz\033[0m "
            "café 界 á \033[1;31mbold red\033[0m \033[48;5;236mbg\033[0m\n")
    return (line * 500_000).encode()


def redraw():
    # 10,000 full-screen frames in the alternate screen, like a busy TUI (btop/htop).
    frames = []
    for f in range(10000):
        parts = ["\033[H"]
        for r in range(rows):
            color = 16 + (r + f) % 216
            parts.append(f"\033[{r + 1};1H\033[38;5;{color}m")
            parts.append(chr(0x41 + (r + f) % 26) * (cols - 1))
        frames.append("".join(parts))
    return ("\033[?1049h" + "".join(frames) + "\033[0m\033[?1049l").encode()


data = {"plain": plain, "unicode": unicode, "redraw": redraw}[mode]()
fd = sys.stdout.fileno()
start = time.monotonic()
view = memoryview(data)
while view:
    view = view[os.write(fd, view):]
termios.tcdrain(fd)  # wait until the terminal has read everything
elapsed = time.monotonic() - start
with open(out, "w") as f:
    f.write(f"{elapsed} {len(data)} {rows} {cols}\n")
