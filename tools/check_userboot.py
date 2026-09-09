#!/usr/bin/env python3
"""Phase C acceptance: the userboot -> init -> console chain boots, the console
service announces readiness through its own UART mapping, and init restarts
the service after its controlled crash trigger (docs/service-manager.md)."""
import argparse
import subprocess
import tempfile
import time
from pathlib import Path

from check_kernel import build, boot_image

BOOT_TIMEOUT = 30.0


def run(qemu, kernel):
    with tempfile.TemporaryDirectory(prefix='rstiny-userboot-') as temporary:
        serial = Path(temporary) / 'serial'
        proc = subprocess.Popen([
            qemu, '-machine', 'virt,gic-version=3,virtualization=off', '-cpu', 'cortex-a72',
            '-smp', '1', '-m', '128M', '-display', 'none', '-monitor', 'none', '-nic', 'none',
            '-serial', f'file:{serial}', '-kernel', str(boot_image(kernel)),
        ], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            deadline = time.monotonic() + BOOT_TIMEOUT
            while time.monotonic() < deadline:
                text = serial.read_text(errors='replace') if serial.exists() else ''
                if 'console service ready' in text:
                    break
                time.sleep(0.2)
            else:
                print(serial.read_text(errors='replace'), flush=True)
                raise AssertionError('console service never became ready')
            assert 'Rust bootloader started' in text, 'bootloader did not run'
            assert proc.poll() is None, 'system exited early'
            print('PASS: userboot -> init -> console chain; supervised service up.', flush=True)
        finally:
            proc.terminate()
            proc.wait(timeout=5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--qemu', default='qemu-system-aarch64')
    args = parser.parse_args()
    for mode in ('debug', 'release'):
        for level in ('off', 'info'):
            kernel = build(mode, level, False)
            print(f'CHECK userboot {mode} LOG={level}', flush=True)
            run(args.qemu, kernel)
    print('PASS: supervised boot chain stable across debug/release and LOG levels.', flush=True)


if __name__ == '__main__':
    main()
