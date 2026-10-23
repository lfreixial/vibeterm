#!/usr/bin/env python3
"""Deterministic terminal rendering fixture. Prints real ANSI/Unicode output."""
print("\033[2J\033[H", end="")
print("\033[38;2;128;203;196m\033[1m  V I B E T E R M\033[0m")
print("  Native Linux terminal · rendering fixture\n")
print("  \033[1mReadable by default.\033[0m  \033[3mBuilt for the keyboard.\033[0m")
print("  \033[4mUnderline\033[0m  \033[9mStrikeout\033[0m  \033[7m Inverse \033[0m  \033[2mDim\033[0m\n")
print("  ANSI palette")
for start in (0, 8):
    print("  " + "".join(f"\033[48;5;{i}m    " for i in range(start, start + 8)) + "\033[0m")
print("\n  256-color spectrum")
for start in range(16, 232, 36):
    print("  " + "".join(f"\033[48;5;{i}m  " for i in range(start, start + 36)) + "\033[0m")
print("\n  Truecolor gradient")
print("  " + "".join(f"\033[48;2;{40+i};{100+i*2};{160+i}m " for i in range(72)) + "\033[0m")
print("\n  UTF-8 / fallback / cell alignment")
print("  café  naïve  a\u0301 e\u0308  λ → ∞  界面  日本語  🦀 🚀")
print("  ┌────────────────────────────┐")
print("  │  cells stay on their grid  │")
print("  └────────────────────────────┘")
print("\n  \033[38;2;128;203;196m❯\033[0m GPU text • real PTY • bounded scrollback", flush=True)
