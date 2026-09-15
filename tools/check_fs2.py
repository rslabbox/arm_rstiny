#!/usr/bin/env python3
"""fs protocol v2 acceptance (docs/roadmap-next.md P2.1).

Builds a FAT32 disk that carries a >8.3 long-name file (mtools stores it as a
FAT long-name record; hadris reads it back), then drives mysh over the serial
line:

  1. `cat <long-name>` -> the fs v2 OPEN carries the whole name through the
     IPC buffer (P0 long-message correctness) and the file contents print;
  2. `./minic one two` -> minic binds the fs service as a second client while
     mysh stays bound: two entries in the server's binding table, each with
     its own shared-buffer page, served concurrently;
  3. `cat APPS.CFG` -> the first client is still served correctly after the
     second client bound, read and exited;
  4. `exit` powers the machine off.

With LOG=info the serial also shows the server assigning both bind slots
(`[fs] client ... bound as #0/#1`).
"""
import argparse
import os
import select
import subprocess
import tempfile
import time
from pathlib import Path

from check_kernel import boot_image

BOOT_TIMEOUT = 400.0
TARGET = 'aarch64-unknown-none-softfloat'
PROMPT = b'[rstiny ~]$: '
LONG_NAME = 'a-very-long-filename.txt'
LONG_CONTENT = b'fs-v2-long-name-ok\nsecond line of the long-name file\n'


def run(qemu, kernel, disk, expect_bind_slots):
    args = [
        qemu, '-machine', 'virt,gic-version=3,virtualization=off', '-cpu', 'cortex-a72',
        '-smp', '1', '-m', '128M', '-display', 'none', '-monitor', 'none', '-nic', 'none',
        '-global', 'virtio-mmio.force-legacy=false',
        '-drive', f'file={disk},if=none,format=raw,id=hd0,readonly=on',
        '-device', 'virtio-blk-device,drive=hd0',
        '-device', 'virtio-gpu-device,xres=640,yres=480',
        '-device', 'virtio-keyboard-device',
        '-device', 'virtio-mouse-device',
        '-serial', 'stdio', '-kernel', str(boot_image(kernel)),
    ]
    proc = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT)
    try:
        commands = [
            b'ls\n',
            f'cat {LONG_NAME}\n'.encode(),
            b'./minic one two\n',
            b'cat APPS.CFG\n',
            b'exit\n',
        ]
        sent = 0
        text = b''
        deadline = time.monotonic() + BOOT_TIMEOUT
        while time.monotonic() < deadline:
            ready, _, _ = select.select([proc.stdout], [], [], 0.3)
            if ready:
                chunk = os.read(proc.stdout.fileno(), 4096)
                if not chunk:
                    break
                text += chunk
            if sent < len(commands) and text.count(PROMPT) > sent:
                proc.stdin.write(commands[sent])
                proc.stdin.flush()
                sent += 1
            if b'[mysh] bye' in text:
                break
        decoded = text.decode(errors='replace')
        print(decoded, flush=True)
        assert sent == len(commands), 'the shell never reached every command'
        assert '[mysh] ready' in decoded, 'mysh never bound the fs service'
        assert 'fs-v2-long-name-ok' in decoded, \
            f'long-name open/read failed (fs v2 long names)'
        assert 'second line of the long-name file' in decoded, 'long-name read truncated'
        assert decoded.count('[mysh] ./minic exited: 0') == 1, 'minic must clean-exit'
        assert '[minic] fs bound' in decoded and 'APPS.CFG size=' in decoded, \
            'the second fs client never bound or read'
        assert '# APPS.CFG' in decoded, \
            'the first fs client must still be served after a second one bound'
        assert 'kernel panic' not in decoded and 'panicked' not in decoded
        if expect_bind_slots:
            assert '[fs] client' in decoded and 'bound as #0' in decoded, \
                'the server did not log the first bind slot'
            assert 'bound as #1' in decoded, \
                'the second client did not get its own binding slot'
        try:
            rc = proc.wait(timeout=15)
        except subprocess.TimeoutExpired:
            raise AssertionError('exit did not power the machine off')
        assert rc == 0, f'unexpected qemu exit code {rc}'
        print('PASS: fs v2 long-name open/read, two concurrent bound clients, '
              'first client still served, then powered off.', flush=True)
    finally:
        if proc.poll() is None:
            proc.terminate()
            proc.wait(timeout=5)


def build_and_make_disk(root, mode, level):
    subprocess.run(['make', 'build', f'MODE={mode}', f'LOG={level}', 'DISK=1'],
                   cwd=root, check=True, stdout=subprocess.DEVNULL)
    disk = root / f'target/apps/{mode}/disk-fs2.img'
    with tempfile.NamedTemporaryFile('wb', suffix='.txt', delete=False) as long_file:
        long_file.write(LONG_CONTENT)
        long_path = long_file.name
    try:
        subprocess.run(['python3', str(root / 'tools/make_disk.py'), str(disk),
                        '--file', f'{LONG_NAME}={long_path}',
                        '--file', f'minic={root}/target/apps/{mode}/minic.elf',
                        '--file', f'APPS.CFG={root}/configs/APPS.CFG'],
                       cwd=root, check=True, stdout=subprocess.DEVNULL)
    finally:
        os.unlink(long_path)
    return root / f'target/kernel/{mode}-log{level}-test0/{TARGET}/{mode}/kernel'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--qemu', default='qemu-system-aarch64')
    parser.add_argument('--mode', choices=('debug', 'release'), default=None)
    parser.add_argument('--level', choices=('off', 'info'), default=None)
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    modes = [args.mode] if args.mode else ('debug', 'release')
    levels = [args.level] if args.level else ('off', 'info')
    for mode in modes:
        for level in levels:
            kernel = build_and_make_disk(root, mode, level)
            disk = root / f'target/apps/{mode}/disk-fs2.img'
            print(f'CHECK fs2 {mode} LOG={level}: long names + two clients', flush=True)
            run(args.qemu, kernel, disk, expect_bind_slots=(level == 'info'))
    print('PASS: fs v2 serves long names and concurrent clients.', flush=True)


if __name__ == '__main__':
    main()
