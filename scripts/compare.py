#!/usr/bin/env python3
"""Compare terminals on output throughput, startup and idle resources.
Usage: compare.py [VIBETERM_BINARY] [RUNS]; installed terminals are found on PATH."""
import json
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

here = Path(__file__).resolve().parent
inner = here / "compare_workload.py"
vibeterm = str(Path(sys.argv[1] if len(sys.argv) > 1 else "target/release/vibeterm").resolve())
runs = int(sys.argv[2]) if len(sys.argv) > 2 else 5
work = Path(tempfile.mkdtemp(prefix="termbench-"))
empty = work / "empty.toml"
empty.write_text("")

# Each entry turns a shell command string into an argv that runs it and then exits.
# Default settings everywhere: personal configs are bypassed.
terminals = {
    "vibeterm": lambda cmd: [vibeterm, "--config", str(empty), "-e", "/bin/sh", "-c", cmd],
    "alacritty": lambda cmd: ["alacritty", "--config-file", str(empty),
                                  "-o", "window.dimensions.columns=110", "-o", "window.dimensions.lines=32", "-e", "/bin/sh", "-c", cmd],
    "kitty": lambda cmd: ["kitty", "--config", "NONE", "-o", "remember_window_size=no",
                              "-o", "initial_window_width=110c", "-o", "initial_window_height=32c", "/bin/sh", "-c", cmd],
    "konsole": lambda cmd: ["konsole", "--separate", "--nofork",
                                "-p", "TerminalColumns=110", "-p", "TerminalRows=32", "-e", "/bin/sh", "-c", cmd],
}
terminals = {k: v for k, v in terminals.items() if k == "vibeterm" or shutil.which(k)}
env = dict(os.environ)
for key in ("TMUX", "KITTY_WINDOW_ID", "ALACRITTY_SOCKET"):
    env.pop(key, None)


def launch(name, cmd, timeout=120):
    return subprocess.run(terminals[name](cmd), env=env, timeout=timeout,
                          stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


results = {name: {} for name in terminals}

for mode in ("plain", "unicode", "redraw"):
    for name in terminals:
        times, grids = [], set()
        for i in range(runs):
            out = work / f"{name}-{mode}-{i}.txt"
            launch(name, f"python3 -I {inner} {mode} {out}")
            if not out.exists():
                print(f"{name} {mode} run {i}: no result", flush=True)
                continue
            elapsed, size, rows, cols = out.read_text().split()
            times.append(float(elapsed))
            grids.add(f"{cols}x{rows}")
        if times:
            median = statistics.median(times)
            results[name][mode] = {"median_s": median, "runs": times,
                                   "mib_s": int(size) / median / 2**20, "grid": sorted(grids)}
            print(f"{mode:8} {name:10} {median:7.3f}s  {int(size) / median / 2**20:7.1f} MiB/s  grid {sorted(grids)}", flush=True)

for name in terminals:
    times = []
    for i in range(max(runs * 2, 10)):
        out = work / f"{name}-start-{i}.txt"
        start = time.time()
        launch(name, f"date +%s.%N > {out}")
        if out.exists():
            times.append(float(out.read_text()) - start)
        time.sleep(0.3)
    results[name]["startup"] = {"median_s": statistics.median(times), "runs": times}
    print(f"startup  {name:10} {statistics.median(times) * 1000:7.1f} ms", flush=True)


def proc_stats(pid):
    fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
    cpu = (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK")
    rollup = Path(f"/proc/{pid}/smaps_rollup").read_text()
    kib = {line.split(":")[0]: int(line.split()[1]) for line in rollup.splitlines()[1:]}
    return cpu, kib["Rss"], kib["Pss"]


for name in terminals:
    process = subprocess.Popen(terminals[name]("printf 'idle\\n'; sleep 9"), env=env,
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(3)
    cpu0, _, _ = proc_stats(process.pid)
    t0 = time.monotonic()
    time.sleep(5)
    cpu1, rss, pss = proc_stats(process.pid)
    cpu = 100 * (cpu1 - cpu0) / (time.monotonic() - t0)
    process.wait(timeout=30)
    results[name]["idle"] = {"cpu_percent": cpu, "rss_kib": rss, "pss_kib": pss}
    print(f"idle     {name:10} cpu {cpu:5.2f}%  RSS {rss / 1024:6.1f} MiB  PSS {pss / 1024:6.1f} MiB", flush=True)

(work / "results.json").write_text(json.dumps(results, indent=2))
print(f"Results: {work / 'results.json'}")
