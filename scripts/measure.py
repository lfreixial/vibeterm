#!/usr/bin/env python3
"""Small reproducible baseline; never compares against other terminals."""
import os
from pathlib import Path
import subprocess
import sys
import time

binary = str(Path(sys.argv[1] if len(sys.argv) > 1 else "target/release/vibeterm").resolve())
root = Path(__file__).resolve().parent.parent

def stats(pid):
    fields = Path(f"/proc/{pid}/stat").read_text().split()
    cpu = (int(fields[13]) + int(fields[14])) / os.sysconf("SC_CLK_TCK")
    rss = next(line for line in Path(f"/proc/{pid}/status").read_text().splitlines() if line.startswith("VmRSS:"))
    return cpu, rss

process = subprocess.Popen([binary, "--diagnostics", "--config", str(root / "config.example.toml"), "-e", "/bin/sh", "-c", "printf 'Idle measurement: cursor blink off\\n'; sleep 7"], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
time.sleep(2)
start_cpu, _ = stats(process.pid)
start = time.monotonic()
time.sleep(4)
end_cpu, rss = stats(process.pid)
elapsed = time.monotonic() - start
stdout, stderr = process.communicate(timeout=10)
assert process.returncode == 0, stderr
print(f"IDLE: {elapsed:.3f}s sample after 2s warmup; main-process CPU={100 * (end_cpu - start_cpu) / elapsed:.3f}% of one core; {rss}")
print(stderr.strip())

for iteration in range(3):
    result = subprocess.run([binary, "--diagnostics", "-e", "/bin/sh", "-c", "printf 'Startup fixture\\n'; sleep 0.4"], text=True, capture_output=True, check=True)
    print(f"WARM START {iteration + 1}:\n{result.stderr.strip()}")

start = time.monotonic()
result = subprocess.run([binary, "--diagnostics", "-d", str(root), "-e", "python3", "scripts/throughput.py", "1000000"], text=True, capture_output=True, check=True)
print(f"OUTPUT: 1,000,000 fixed lines; process wall time={time.monotonic() - start:.3f}s (includes startup/exit)")
print(result.stderr.strip())
