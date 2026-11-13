"""Background-pane interaction regressions, run with x11-smoke.py --lifecycle."""
from pathlib import Path
import subprocess
import time


def run(work, config, key, type_text, wait_for, binary, env):
    shell = work / 'lifecycle-shell'
    shell.write_text('#!/bin/sh\nexec /bin/bash --noprofile --norc\n')
    shell.chmod(0o755)
    config.write_text(f'font_size = 15\nshell = "{shell}"\n')
    time.sleep(0.4)
    helper = Path(binary).parent / 'examples/clipboard_probe'
    assert helper.exists(), 'Build --examples before lifecycle checks'
    type_text('printf "%s" $$ > original-pid.txt\n')
    wait_for(lambda: (work / 'original-pid.txt').exists(), 'Original shell not ready')
    original = (work / 'original-pid.txt').read_text()

    def background(kind, tag):
        key(ord('T' if kind == 'tab' else 'E'), 5)
        time.sleep(0.5)
        type_text(f'printf "%s" $$ > {tag}.pid; while [ ! -e {tag}.exit ]; do sleep 0.05; done; exit\n')
        pidfile = work / f'{tag}.pid'
        wait_for(lambda: pidfile.exists() and pidfile.read_text(), 'Background shell not ready')
        pid = pidfile.read_text()
        key(0xff09 if kind == 'tab' else 0xff51, 5)
        time.sleep(0.2)
        return pid

    def finish(tag, pid):
        (work / f'{tag}.exit').touch()
        wait_for(lambda: not Path(f'/proc/{pid}').exists(), 'Background shell did not exit')
        # Allow the exit event to reach the UI before continuing the interaction.
        time.sleep(0.4)

    for kind in ('tab', 'split'):
        tag = f'{kind}-search'
        pid = background(kind, tag)
        # Assemble the needle so the echoed command cannot match it.
        type_text("printf 'lifecycle-%s\\n' 'needle'\n")
        time.sleep(0.2)
        owner = subprocess.Popen([str(helper), 'copy', 'no-selection'], env=env,
                                 stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
        try:
            assert owner.stdout.readline().strip() == 'ready'
            key(ord('F'), 5)
            type_text('lifecycle-')
            finish(tag, pid)
            type_text('needle')
            time.sleep(0.3)
            key(0xff0d)  # Retry the search if a background output revision cancelled it.
            time.sleep(0.2)
            key(ord('C'), 5)
            wait_for(lambda: subprocess.check_output([str(helper)], env=env, text=True).strip()
                     == 'lifecycle-needle', 'Search was cancelled by background exit')
        finally:
            owner.communicate('done\n', timeout=3)
        key(0xff1b)
        print(f'PASS: search survives background {kind} exit', flush=True)

        tag = f'{kind}-palette'
        pid = background(kind, tag)
        key(ord('P'), 5)
        type_text('New ')
        finish(tag, pid)
        type_text('tab')
        key(0xff0d)
        time.sleep(0.5)
        type_text(f'printf "%s" $$ > {tag}-new.pid\n')
        new_pid = work / f'{tag}-new.pid'
        wait_for(lambda: new_pid.exists() and new_pid.read_text(), 'Palette command did not finish')
        assert new_pid.read_text() != original, 'Palette was cancelled; command ran in original shell'
        key(ord('W'), 5)
        time.sleep(0.2)
        print(f'PASS: palette survives background {kind} exit and opens a new tab', flush=True)
