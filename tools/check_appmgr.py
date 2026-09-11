#!/usr/bin/env python3
"""Phase D3 acceptance: appmgr reads APPS.CFG from the FAT32 disk, loads
HELLO.ELF from the same disk and supervises it. Replacing the on-disk ELF
changes what runs without rebuilding the system image (docs/disk-driver.md
section 12, D3)."""
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


def run(qemu, kernel, disk, message):
    with tempfile.TemporaryDirectory(prefix='rstiny-appmgr-') as temporary:
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
                if message in text:
                    break
                time.sleep(0.2)
            else:
                print(text, flush=True)
                raise AssertionError(f'{message!r} never appeared on the console')
            # The reap line lands shortly after the app's output.
            stop_deadline = time.monotonic() + 30.0
            while time.monotonic() < stop_deadline:
                text = serial.read_text(errors='replace') if serial.exists() else ''
                if '[appmgr] app stopped: hello' in text:
                    break
                time.sleep(0.2)
            assert '[appmgr] app started: hello' in text, 'appmgr did not confirm the app'
            assert '[appmgr] app stopped: hello' in text, 'the clean exit was not reaped'
            assert 'kernel panic' not in text and 'panicked' not in text
            assert proc.poll() is None, 'system exited early'
            print(f'PASS: hello loaded from disk and supervised ({message}).', flush=True)
        finally:
            proc.terminate()
            proc.wait(timeout=5)


def build_and_make_disk(root, mode, level, message_env):
    env = dict(os.environ)
    if message_env is not None:
        env['HELLO_MSG'] = message_env
    subprocess.run(['make', 'build', f'MODE={mode}', f'LOG={level}', 'DISK=1',
                    'INIT_CFG=apps/init-appmgr.cfg', 'APPS_CFG=apps/APPS-hello.CFG'],
                   cwd=root, check=True, stdout=subprocess.DEVNULL, env=env)
    subprocess.run(['make', 'disk', f'MODE={mode}', 'APPS_CFG=apps/APPS-hello.CFG'], cwd=root,
                   check=True, stdout=subprocess.DEVNULL, env=env)
    return root / f'target/kernel/{mode}-log{level}-test0/{TARGET}/{mode}/kernel'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--qemu', default='qemu-system-aarch64')
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    for mode in ('debug', 'release'):
        for level in ('off', 'info'):
            kernel = build_and_make_disk(root, mode, level, None)
            disk = root / f'target/apps/{mode}/disk.img'
            print(f'CHECK appmgr {mode} LOG={level}: stock image', flush=True)
            run(args.qemu, kernel, disk, DEFAULT_MESSAGE)

            # Replace the ELF on the disk only: the system image is untouched.
            replacement = '[hello] replaced on disk; no rebuild needed'
            kernel = build_and_make_disk(root, mode, level, replacement)
            print(f'CHECK appmgr {mode} LOG={level}: swapped ELF', flush=True)
            run(args.qemu, kernel, disk, replacement)
    print('PASS: disk-loaded apps run under appmgr and follow the disk content.', flush=True)


if __name__ == '__main__':
    main()
