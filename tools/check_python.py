#!/usr/bin/env python3
"""MicroPython port acceptance (docs/micropython-port.md §10, 阶段 P3).

Boots the stock topology (console/block/fs/mysh) with `python.elf` and a
script `APP.PY` on the FAT32 disk, then drives the REPL over the serial line:

  1. `./python` -> a `MicroPython v1.24.x` banner and the `>>>` prompt;
     `print(1+2)` evaluates to `3`, and Ctrl-D ends the REPL so mysh reaps
     `[mysh] ./python exited: 0`;
  2. `./python app` -> the fs-backed script path (决策 I: slot 53 fs grant +
     ArgvBlock for "app"): `print('hi from disk')` and `print(6*7)` reach the
     console, then a clean `exited: 0`;
  3. `exit` powers the machine off.

Every keystroke is sent immediately after its trigger output appears (the
serial harness drives the guest while it is actively emitting; see the same
convention in check_mysh.py). REPL lines end with `\\r` (MicroPython readline
treats CR as Enter); mysh commands end with `\\n`.
"""
import argparse
import os
import select
import subprocess
import tempfile
import time
from pathlib import Path

from check_kernel import boot_image

BOOT_TIMEOUT = 300.0
TARGET = 'aarch64-unknown-none-softfloat'


def run(qemu, kernel, disk):
    args = [
        qemu, '-machine', 'virt,gic-version=3,virtualization=off', '-cpu', 'cortex-a72',
        '-smp', '1', '-m', '128M', '-display', 'none', '-monitor', 'none', '-nic', 'none',
        '-global', 'virtio-mmio.force-legacy=false',
        '-drive', f'file={disk},if=none,format=raw,id=hd0,readonly=on',
        '-device', 'virtio-blk-device,drive=hd0',
        '-serial', 'stdio', '-kernel', str(boot_image(kernel)),
    ]
    proc = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT)
    # (trigger marker, input to send, acknowledgement to wait for).  The
    # harness always sends immediately after its trigger output appears: the
    # serial pipe only stays live once at least one byte has moved during an
    # active-output window (see the x/y wake-up experiment), so each send is
    # glued to the newest output event.  No retry: a duplicated REPL/mysh
    # command would corrupt the session (e.g. Ctrl-D is mysh EOF).
    stages = [
        (b'[rstiny ~]$: ', b'./python\n', b'MicroPython v'),
        (b'>>> ', b'print(1+2)\r', b'3'),
        (b'>>> ', b'\x04', b'[mysh] ./python exited: 0'),   # Ctrl-D ends REPL
        (b'[mysh] ./python exited: 0', b'./python app\n', b'hi from disk'),
        (b'hi from disk', b'', b'42'),                        # second script print
        (b'[mysh] ./python exited: 0', b'exit\n', b'[mysh] bye'),
    ]
    try:
        text = b''
        seen = [0] * len(stages)
        stage_state = ['wait-trigger'] * len(stages)
        ack = [None] * len(stages)
        deadline = time.monotonic() + BOOT_TIMEOUT
        while time.monotonic() < deadline:
            ready, _, _ = select.select([proc.stdout], [], [], 0.3)
            if ready:
                chunk = os.read(proc.stdout.fileno(), 4096)
                if not chunk:
                    break
                text += chunk
            for i, (trigger, command, ack_marker) in enumerate(stages):
                if stage_state[i] == 'done':
                    continue
                if stage_state[i] == 'wait-trigger':
                    count = text.count(trigger)
                    if count > seen[i]:
                        seen[i] = count
                        if command:
                            proc.stdin.write(command)
                            proc.stdin.flush()
                        if ack_marker is None:
                            stage_state[i] = 'done'
                        else:
                            stage_state[i] = 'wait-ack'
                            ack[i] = (ack_marker, text.count(ack_marker))
                        break
                else:
                    marker, base = ack[i]
                    if text.count(marker) > base:
                        stage_state[i] = 'done'
            if b'[mysh] bye' in text:
                break
        decoded = text.decode(errors='replace')
        print(decoded, flush=True)
        assert all(s == 'done' for s in stage_state), \
            f'not every stage completed: {stage_state} (acks {ack})'
        assert 'MicroPython v1.24' in decoded, 'REPL banner missing'
        assert '>>> ' in decoded, 'REPL prompt missing'
        assert 'print(1+2)' in decoded, 'print(1+2) echo missing'
        assert decoded.count('[mysh] ./python exited: 0') >= 2, \
            'both runs must be reaped with exit code 0'
        assert 'hi from disk' in decoded, 'script print did not reach the console'
        assert '42' in decoded, 'script arithmetic print missing'
        assert 'kernel panic' not in decoded and 'panicked' not in decoded
        try:
            rc = proc.wait(timeout=20)
        except subprocess.TimeoutExpired:
            raise AssertionError('exit did not power the machine off')
        assert rc == 0, f'unexpected qemu exit code {rc}'
        print('PASS: python REPL and ./python app script run, then powered off.', flush=True)
    finally:
        if proc.poll() is None:
            proc.terminate()
            proc.wait(timeout=5)


def build_and_make_disk(root, mode, level):
    subprocess.run(['make', 'build', f'MODE={mode}', f'LOG={level}', 'DISK=1'],
                   cwd=root, check=True, stdout=subprocess.DEVNULL)
    subprocess.run(['make', 'disk', f'MODE={mode}'], cwd=root, check=True,
                   stdout=subprocess.DEVNULL)
    return root / f'target/kernel/{mode}-log{level}-test0/{TARGET}/{mode}/kernel'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--qemu', default='qemu-system-aarch64')
    parser.add_argument('--mode', choices=('debug', 'release'), default=None)
    parser.add_argument('--level', choices=('off', 'info'), default=None)
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    script = b"print('hi from disk')\nprint(6*7)\n"
    modes = [args.mode] if args.mode else ('debug', 'release')
    levels = [args.level] if args.level else ('off', 'info')
    for mode in modes:
        for level in levels:
            kernel = build_and_make_disk(root, mode, level)
            disk = root / f'target/apps/{mode}/disk-python-{level}.img'
            with tempfile.NamedTemporaryFile('wb', delete=False) as app:
                app.write(script)
                app_path = app.name
            try:
                subprocess.run(['python3', str(root / 'tools/make_disk.py'), str(disk),
                                '--file', f'hello={root}/target/apps/{mode}/hello.elf',
                                '--file', f'minic={root}/target/apps/{mode}/minic.elf',
                                '--file', f'python={root}/target/apps/{mode}/python.elf',
                                '--file', f'APP.PY={app_path}',
                                '--file', f'APPS.CFG={root}/configs/APPS.CFG'],
                               cwd=root, check=True, stdout=subprocess.DEVNULL)
            finally:
                os.unlink(app_path)
            print(f'CHECK python {mode} LOG={level}: REPL + ./python app + poweroff',
                  flush=True)
            run(args.qemu, kernel, disk)
    print('PASS: the MicroPython port runs the REPL and disk scripts.', flush=True)


if __name__ == '__main__':
    main()