#!/usr/bin/env python3
"""Phase D1 acceptance: the block-server probes the VirtIO MMIO device,
reports the disk capacity, and reads sector 0 into the shared buffer. The
checksum is compared against a host-side read of the same image
(docs/disk-driver.md section 12, D1)."""
import argparse
import subprocess
import tempfile
import time
from pathlib import Path

from check_kernel import boot_image

BOOT_TIMEOUT = 30.0
TARGET = 'aarch64-unknown-none-softfloat'


def run(qemu, kernel, disk):
    raw = disk.read_bytes()[:512]
    expected_sum = sum(raw)
    expected_head = int.from_bytes(raw[:4], 'little')
    with tempfile.TemporaryDirectory(prefix='rstiny-block-') as temporary:
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
                if '[block] test sum=' in text:
                    break
                time.sleep(0.2)
            else:
                print(text, flush=True)
                raise AssertionError('block-server never completed its test read')
            assert f'[block] test capacity={len(disk.read_bytes()) // 512}' in text, \
                'capacity does not match the image'
            assert f'[block] test sum={expected_sum:#x}' in text, 'sector 0 checksum mismatch'
            assert f'head={expected_head:#x}' in text, 'sector 0 head mismatch'
            assert proc.poll() is None, 'system exited early'
            print(f'PASS: block-server capacity + sector 0 DMA verified ({disk.name}).', flush=True)
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
            # One make invocation keeps the BLK_TEST hook and the boot image
            # in sync: cargo re-fingerprints option_env! when the flags change.
            subprocess.run(['make', 'build', f'MODE={mode}', f'LOG={level}', 'BLK_TEST=1', 'DISK=1'],
                           cwd=root, check=True, stdout=subprocess.DEVNULL)
            subprocess.run(['make', 'disk', f'MODE={mode}'], cwd=root, check=True,
                           stdout=subprocess.DEVNULL)
            kernel = root / f'target/kernel/{mode}-log{level}-test0/{TARGET}/{mode}/kernel'
            print(f'CHECK block {mode} LOG={level}', flush=True)
            run(args.qemu, kernel, root / f'target/apps/{mode}/disk.img')
    print('PASS: block protocol verified across debug/release and LOG levels.', flush=True)


if __name__ == '__main__':
    main()
