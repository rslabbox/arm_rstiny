#!/usr/bin/env python3
"""Phase D4 acceptance: `mysh` is an interactive shell. It prompts on the
console, lists the FAT32 root directory, prints a file and executes `./hello`
by loading and supervising `HELLO.ELF` from disk.

The shell is the fifth service (console/block/fs/appmgr/mysh). Console RX is
polling-only, so the harness drives the guest over the serial line and waits
for each prompt before typing the next command.
"""
import argparse
import os
import select
import subprocess
import tempfile
import time
from pathlib import Path

from check_kernel import boot_image

BOOT_TIMEOUT = 240.0
TARGET = 'aarch64-unknown-none-softfloat'
PROMPT = b'[rstiny ~]$: '
DEFAULT_MESSAGE = '[hello] Hello, world! (loaded from disk)'


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
    try:
        commands = [b'ls\n', b'./hello\n', b'cat APPS.CFG\n', b'exit\n']
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
            # Each fresh prompt means the shell consumed the previous command.
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
        assert '[rstiny ~]$: ' in decoded, 'the shell never printed a prompt'
        # `ls`: the two files the disk image carries.
        assert 'HELLO.ELF' in decoded, 'ls did not show HELLO.ELF'
        assert 'APPS.CFG' in decoded, 'ls did not show APPS.CFG'
        # `cat APPS.CFG`: the manifest's first line reaches the console.
        assert '# APPS.CFG' in decoded, 'cat did not print the file contents'
        # `./hello`: the shell loads the ELF, replies READY, reaps EXIT.
        assert '[mysh] ./hello' in decoded, 'the shell never started hello'
        assert DEFAULT_MESSAGE in decoded, 'hello did not run under mysh'
        assert '[mysh] hello exited: 0' in decoded, 'the clean exit was not reaped'
        assert 'kernel panic' not in decoded and 'panicked' not in decoded
        assert proc.poll() is None, 'system exited early'
        print('PASS: mysh ran ls, cat and ./hello from interactive input.', flush=True)
    finally:
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
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    for mode in ('debug', 'release'):
        for level in ('off', 'info'):
            kernel = build_and_make_disk(root, mode, level)
            disk = root / f'target/apps/{mode}/disk.img'
            print(f'CHECK mysh {mode} LOG={level}', flush=True)
            run(args.qemu, kernel, disk)
    print('PASS: the interactive shell drives the fs service and supervises ./hello.',
          flush=True)


if __name__ == '__main__':
    main()
