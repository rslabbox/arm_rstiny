#!/usr/bin/env python3
"""Phase D5 acceptance: killing fs-server exercises the whole dependency
chain — init reaps the crashed service, notifies and restarts its dependents,
appmgr tears down its apps, and the rebuilt chain loads hello from the disk a
second time with no frame budget leaked (docs/disk-driver.md section 12, D5)."""
import argparse
import os
import re
import subprocess
import tempfile
import time
from pathlib import Path

from check_kernel import boot_image

BOOT_TIMEOUT = 420.0
TARGET = 'aarch64-unknown-none-softfloat'


def run(qemu, kernel, disk):
    with tempfile.TemporaryDirectory(prefix='rstiny-restart-') as temporary:
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
                if text.count('[hello] Hello') >= 2:
                    break
                time.sleep(0.2)
            else:
                print(text, flush=True)
                raise AssertionError('hello did not run twice after the fs crash')
            assert '[init] crash drill: reaping fs' in text, 'the crash drill did not trigger'
            frames = re.findall(r'\[appmgr\] frames=(\d+)', text)
            assert len(frames) >= 2, 'appmgr did not restart'
            assert frames[0] == frames[1], f'frame budget leaked: {frames[0]} -> {frames[1]}'
            assert 'kernel panic' not in text and 'panicked' not in text
            assert proc.poll() is None, 'system exited early'
            print(f'PASS: crash -> notify -> rebuild chain; frames stable at {frames[0]}.',
                  flush=True)
        finally:
            proc.terminate()
            proc.wait(timeout=5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--qemu', default='qemu-system-aarch64')
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    for mode in ('debug', 'release'):
        for level in ('off', 'info'):
            env = dict(os.environ)
            env['KILL_FS'] = '1'
            subprocess.run(['make', 'build', f'MODE={mode}', f'LOG={level}', 'KILL_FS=1', 'DISK=1',
                            'APPS_CFG=configs/APPS-hello.CFG'],
                           cwd=root, check=True, stdout=subprocess.DEVNULL, env=env)
            subprocess.run(['make', 'disk', f'MODE={mode}', 'APPS_CFG=configs/APPS-hello.CFG'],
                           cwd=root, check=True, stdout=subprocess.DEVNULL, env=env)
            kernel = root / f'target/kernel/{mode}-log{level}-test0/{TARGET}/{mode}/kernel'
            disk = root / f'target/apps/{mode}/disk.img'
            print(f'CHECK restart {mode} LOG={level}', flush=True)
            run(args.qemu, kernel, disk)
    print('PASS: fs crash rebuilds the whole chain without leaks.', flush=True)


if __name__ == '__main__':
    main()
