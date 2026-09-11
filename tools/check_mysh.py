#!/usr/bin/env python3
"""Phase D4 acceptance: `mysh` reads a command script (`SH.CFG`) from the
FAT32 disk, lists the root directory, prints a file, and executes `./hello`
by loading and supervising `HELLO.ELF` from the same disk.

The shell is the fifth service (console/block/fs/appmgr/mysh); it drives the
fs protocol directly and reuses the loader appmgr uses for supervised children
(docs/disk-driver.md, docs/service-manager.md).
"""
import argparse
import os
import subprocess
import tempfile
import time
from pathlib import Path

from check_kernel import boot_image

BOOT_TIMEOUT = 240.0
TARGET = 'aarch64-unknown-none-softfloat'
DEFAULT_MESSAGE = '[hello] Hello, world! (loaded from disk)'


def run(qemu, kernel, disk):
    with tempfile.TemporaryDirectory(prefix='rstiny-mysh-') as temporary:
        serial = Path(temporary) / 'serial'
        proc = subprocess.Popen([
            qemu, '-machine', 'virt,gic-version=3,virtualization=off', '-cpu', 'cortex-a72',
            '-smp', '1', '-m', '128M', '-display', 'none', '-monitor', 'none', '-nic', 'none',
            '-global', 'virtio-mmio.force-legacy=false',
            '-drive', f'file={disk},if=none,format=raw,id=hd0,readonly=on',
            '-device', 'virtio-blk-device,drive=hd0',
            '-serial', f'file:{serial}', '-kernel', str(boot_image(kernel)),
        ], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            deadline = time.monotonic() + BOOT_TIMEOUT
            text = ''
            while time.monotonic() < deadline:
                text = serial.read_text(errors='replace') if serial.exists() else ''
                if '[mysh] done' in text:
                    break
                time.sleep(0.2)
            else:
                print(text, flush=True)
                raise AssertionError('[mysh] done never appeared on the console')
            assert '[mysh] ready' in text, 'mysh never bound the fs service'
            # `ls`: the three files the disk image carries.
            assert 'HELLO.ELF' in text, 'ls did not show HELLO.ELF'
            assert 'APPS.CFG' in text, 'ls did not show APPS.CFG'
            assert 'SH.CFG' in text, 'ls did not show SH.CFG'
            # `cat APPS.CFG`: the manifest's first line reaches the console.
            assert '# APPS.CFG' in text, 'cat did not print the file contents'
            # `./hello`: the shell loads the ELF, replies READY, reaps EXIT.
            assert '[mysh] ./hello' in text, 'the shell never started hello'
            assert DEFAULT_MESSAGE in text, 'hello did not run under mysh'
            assert '[mysh] hello exited: 0' in text, 'the clean exit was not reaped'
            assert 'kernel panic' not in text and 'panicked' not in text
            assert proc.poll() is None, 'system exited early'
            print('PASS: mysh listed the disk, printed a file and ran hello.', flush=True)
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
    print('PASS: the shell drives the fs service and supervises ./hello.', flush=True)


if __name__ == '__main__':
    main()
