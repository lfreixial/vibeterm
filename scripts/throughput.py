#!/usr/bin/env python3
"""Fixed mixed ASCII/Unicode/ANSI output fixture; pass a line count (default 50000)."""
import sys

count = int(sys.argv[1]) if len(sys.argv) > 1 else 50000
line = "\033[38;2;128;203;196m0123456789 abcdefghijklmnopqrstuvwxyz\033[0m café 界 a\u0301\n"
batch = (line * 100).encode()
for _ in range(count // 100):
    sys.stdout.buffer.write(batch)
sys.stdout.buffer.write((line * (count % 100)).encode())
