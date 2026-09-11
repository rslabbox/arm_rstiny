#!/usr/bin/env python3
"""Phase D4 acceptance: `mysh` is an interactive shell. It prompts on the
console, lists the FAT32 root directory, prints a file, and runs `./<name>`
by loading `<NAME>.ELF` from disk and supervising it. `exit` powers the
machine off (PSCI SYSTEM_OFF), which terminates QEMU.

The shell is the fifth service (console/block/fs/mysh). Console RX is
polling-only, so the harness drives the guest over the serial line and waits
for each prompt before typing the next command. The renamed-program phase
runs the same ELF installed as `test` via `./test`.
"""
import argparse
import os
import select
import subprocess
import time
from pathlib import Path

from check_kernel import boot_image

BOOT_TIMEOUT = 240.0
TARGET = 'aarch64-unknown-none-softfloat'
PROMPT = b'[rstiny ~]$: '
DEFAULT_MESSAGE = '[hello] Hello, world! (loaded from disk)'


def run(qemu, kernel, disk, program, expect_poweroff):
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
        commands = [b'ls\n', f'./{program}\n'.encode(), b'cat APPS.CFG\n', b'exit\n']
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
        # `ls`: the files the disk image carries.
        assert 'APPS.CFG' in decoded, 'ls did not show APPS.CFG'
        # `cat APPS.CFG`: the manifest's first line reaches the console.
        assert '# APPS.CFG' in decoded, 'cat did not print the file contents'
        # `./<program>`: the shell loads the ELF, replies READY, reaps EXIT.
        assert f'[mysh] ./{program}' in decoded, 'the shell never started the program'
        assert DEFAULT_MESSAGE in decoded, 'the program did not run under mysh'
        assert f'[mysh] ./{program} exited: 0' in decoded, 'the clean exit was not reaped'
        assert 'kernel panic' not in decoded and 'panicked' not in decoded
        if expect_poweroff:
            # `exit` calls PSCI SYSTEM_OFF; QEMU must terminate on its own.
            try:
                rc = proc.wait(timeout=15)
            except subprocess.TimeoutExpired:
                raise AssertionError('exit did not power the machine off')
            print(f'PASS: mysh ran ls, cat and ./{program}, then powered off (rc={rc}).',
                  flush=True)
        else:
            assert proc.poll() is None, 'system exited early'
            print(f'PASS: mysh ran ls, cat and ./{program}.', flush=True)
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


def make_renamed_disk(root, mode):
    """Install the same ELF as `test` so `./test` proves the name mapping."""
    disk = root / f'target/apps/{mode}/disk-renamed.img'
    subprocess.run(['python3', str(root / 'tools/make_disk.py'), str(disk),
                    '--file', f'test={root}/target/apps/{mode}/hello.elf',
                    '--file', f'APPS.CFG={root}/apps/APPS.CFG'],
                   cwd=root, check=True, stdout=subprocess.DEVNULL)
    return disk


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--qemu', default='qemu-system-aarch64')
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    for mode in ('debug', 'release'):
        for level in ('off', 'info'):
            kernel = build_and_make_disk(root, mode, level)
            disk = root / f'target/apps/{mode}/disk.img'
            print(f'CHECK mysh {mode} LOG={level}: ./hello + poweroff', flush=True)
            run(args.qemu, kernel, disk, 'hello', expect_poweroff=True)
            print(f'CHECK mysh {mode} LOG={level}: renamed test as ./test', flush=True)
            run(args.qemu, kernel, make_renamed_disk(root, mode), 'test', expect_poweroff=True)
    print('PASS: the interactive shell runs disk programs and powers off on exit.',
          flush=True)


if __name__ == '__main__':
    main()
