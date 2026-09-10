#!/usr/bin/env python3
"""Acceptance for the independent fault-handler thread (docs/fault-handler.md
§10, phase F4). Builds the system with BOOT_TEST=1, so init's client thread
performs the crashing console write while the supervisor thread only receives
on the control endpoint. The drill passes when the fault is delivered to the
supervisor (the old single-thread design deadlocked here), the supervisor reaps
and restarts the service, the client's Call fails instead of hanging, and the
restarted console serves output again."""
import argparse
import os
import subprocess
import tempfile
import time
from pathlib import Path

from check_kernel import build, boot_image

BOOT_TIMEOUT = 45.0


def run(qemu, kernel, printing):
    with tempfile.TemporaryDirectory(prefix='rstiny-fault-handler-') as temporary:
        serial = Path(temporary) / 'serial'
        proc = subprocess.Popen([
            qemu, '-machine', 'virt,gic-version=3,virtualization=off', '-cpu', 'cortex-a72',
            '-smp', '1', '-m', '128M', '-display', 'none', '-monitor', 'none', '-nic', 'none',
            '-serial', f'file:{serial}', '-kernel', str(boot_image(kernel)),
        ], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            deadline = time.monotonic() + BOOT_TIMEOUT
            text = ''
            while time.monotonic() < deadline:
                text = serial.read_text(errors='replace') if serial.exists() else ''
                if text.count('console service ready') >= 3:
                    break
                assert 'panicked' not in text, 'a component panicked during the drill'
                assert proc.poll() is None, f'system exited early:\n{text}'
                time.sleep(0.2)
            else:
                print(text, flush=True)
                raise AssertionError('the drill never recovered the console service')
            assert proc.poll() is None, 'system exited during the drill'
            assert 'crash requested' in text, 'console never took the drill crash write'
            # The second ready + the second client-written line are only
            # reachable if the supervisor received the fault, reaped and
            # restarted the service: a hang here means the §1 deadlock.
            assert text.count('console service ready') >= 2, 'console never restarted'
            assert text.count('service started: console') >= 2, (
                'console did not serve a client write after the restart')
            # Group-internal supervision (docs/thread-group.md): the logger
            # self-crashes and is rebuilt; "logger rebuilt" is written through
            # the console by the rebuilt thread itself. The third ready is
            # init's second incarnation: userboot destroyed the whole group
            # and rebuilt it, and its console re-carved its UART frame.
            assert 'logger rebuilt' in text, 'the internal thread was never rebuilt'
            assert text.count('console service ready') >= 3, (
                'init was never restarted by userboot after the group destroy')
            assert text.count('service started: console') >= 3, (
                'the restarted init never supervised its console again')
            if printing:
                # Kernel debug console evidence (LOG=off hides it).
                assert 'user fault' in text, 'kernel never recorded the console fault'
                assert 'teardown done' in text, 'supervisor never reaped the crashed service'
                assert 'failed as designed' in text, (
                    'client Call neither failed nor returned: ' + text[text.find('crash'):])
                assert 'internal thread faulted' in text, (
                    'the internal thread fault never reached the supervisor')
                assert 'generation=1' in text, (
                    'userboot never restarted init after the drill exit')
                print('PASS: service and internal-thread faults supervised, logger rebuilt, '
                      'group destroyed and rebuilt by userboot, console recovered.',
                      flush=True)
            else:
                print('PASS: supervision drills completed without hanging.', flush=True)
        finally:
            proc.terminate()
            proc.wait(timeout=5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--qemu', default='qemu-system-aarch64')
    args = parser.parse_args()
    # BOOT_TEST must reach the app compile (option_env!), so build with it set.
    environment = dict(os.environ, BOOT_TEST='1')
    saved = dict(os.environ)
    os.environ.update(environment)
    try:
        for mode in ('debug', 'release'):
            for level in ('info', 'off'):
                print(f'CHECK fault-handler {mode} LOG={level}', flush=True)
                kernel = build(mode, level, False)
                run(args.qemu, kernel, level != 'off')
    finally:
        os.environ.clear()
        os.environ.update(saved)
    print('PASS: supervision drill across debug/release and LOG levels.', flush=True)


if __name__ == '__main__':
    main()
