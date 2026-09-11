#!/usr/bin/env python3
"""Phase D4 acceptance: init starts the configured services in dependency
order (console -> block -> fs -> appmgr), and a block service that cannot
come up keeps its dependents from ever starting (docs/disk-driver.md
section 12, D4)."""
import argparse
import os
import subprocess
import tempfile
import time
from pathlib import Path

from check_kernel import boot_image

BOOT_TIMEOUT = 240.0
TARGET = 'aarch64-unknown-none-softfloat'


def boot(qemu, kernel, disk):
    with tempfile.TemporaryDirectory(prefix='rstiny-services-') as temporary:
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
                # Healthy boot: appmgr reads the manifest and hello runs.
                # Gated boot: block exhausted its restart budget (5 tries) and
                # the chain stops before fs ever starts.
                if ('[appmgr] manifest lists' in text and '[hello] Hello' in text) or (
                        text.count('service restart scheduled') >= 5):
                    break
                time.sleep(0.2)
            return text, proc
        finally:
            proc.terminate()
            proc.wait(timeout=5)


def healthy_order(text):
    markers = ['service started: console', 'service started: block',
               'service started: fs', 'service started: appmgr']
    positions = [text.find(marker) for marker in markers]
    return all(position >= 0 for position in positions) and positions == sorted(positions)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--qemu', default='qemu-system-aarch64')
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    disk = root / 'target/apps/release/disk.img'
    subprocess.run(['make', 'disk', 'MODE=release', 'APPS_CFG=apps/APPS-hello.CFG'], cwd=root,
                   check=True, stdout=subprocess.DEVNULL)
    for mode in ('debug', 'release'):
        for level in ('off', 'info'):
            subprocess.run(['make', 'build', f'MODE={mode}', f'LOG={level}', 'DISK=1',
                            'INIT_CFG=apps/init-appmgr.cfg'],
                           cwd=root, check=True, stdout=subprocess.DEVNULL)
            kernel = root / f'target/kernel/{mode}-log{level}-test0/{TARGET}/{mode}/kernel'
            print(f'CHECK services {mode} LOG={level}: dependency order', flush=True)
            text, _ = boot(args.qemu, kernel, disk)
            assert healthy_order(text), 'services did not start in dependency order:\n' + text
            assert '[fs] mounted' in text, 'fs did not mount'
            assert '[hello] Hello' in text, 'hello did not run'

            # A block service that dies immediately keeps fs and appmgr down.
            env = dict(os.environ)
            env['BLOCK_TEST'] = 'fail'
            subprocess.run(['make', 'build', f'MODE={mode}', f'LOG={level}', 'DISK=1',
                            'INIT_CFG=apps/init-appmgr.cfg', 'BLOCK_TEST=fail'], cwd=root, check=True,
                           stdout=subprocess.DEVNULL, env=env)
            print(f'CHECK services {mode} LOG={level}: block unavailable', flush=True)
            text, _ = boot(args.qemu, kernel, disk)
            assert 'service started: block' in text, 'block never attempted to start'
            assert '[fs] mounted' not in text, 'fs started without block'
            assert '[appmgr] manifest' not in text, 'appmgr started without fs'
            assert 'kernel panic' not in text and 'panicked' not in text
    print('PASS: dependency ordering and failure gating verified.', flush=True)


if __name__ == '__main__':
    main()
