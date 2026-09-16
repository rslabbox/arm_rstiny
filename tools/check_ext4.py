#!/usr/bin/env python3
"""ext4 acceptance: fs-server mounts the journal-less ext4 image through the
block service (lwext4, read-only) and reads hello byte-for-byte. A corrupted
ext4 magic must surface as a bounded service error - never a kernel panic
(docs/disk-driver.md, the ext4 section)."""
import argparse
import subprocess
import tempfile
import time
from pathlib import Path

from check_kernel import boot_image

BOOT_TIMEOUT = 400.0
TARGET = 'aarch64-unknown-none-softfloat'


def run(qemu, kernel, disk, expectation):
    """Boot with `disk` and wait until `expectation(text)` holds or timeout."""
    with tempfile.TemporaryDirectory(prefix='rstiny-ext4-') as temporary:
        serial = Path(temporary) / 'serial'
        proc = subprocess.Popen([
            qemu, '-machine', 'virt,gic-version=3,virtualization=off', '-cpu', 'cortex-a72',
            '-smp', '1', '-m', '128M', '-display', 'none', '-monitor', 'none', '-nic', 'none',
            '-global', 'virtio-mmio.force-legacy=false',
            '-device', 'virtio-blk-device,drive=hd0',
            '-device', 'virtio-gpu-device,xres=640,yres=480',
            '-device', 'virtio-keyboard-device',
            '-device', 'virtio-mouse-device',
            '-drive', f'file={disk},if=none,format=raw,id=hd0,readonly=on',
            '-serial', f'file:{serial}', '-kernel', str(boot_image(kernel)),
        ], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            deadline = time.monotonic() + BOOT_TIMEOUT
            text = ''
            while time.monotonic() < deadline:
                text = serial.read_text(errors='replace') if serial.exists() else ''
                if expectation(text):
                    break
                time.sleep(0.2)
            else:
                print(text, flush=True)
                raise AssertionError('expected fs-server output never appeared')
            assert 'panicked' not in text and 'kernel panic' not in text, \
                'the kernel panicked on the ext4 image'
            assert proc.poll() is None, 'system exited early'
        finally:
            proc.terminate()
            proc.wait(timeout=5)
    return text


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--qemu', default='qemu-system-aarch64')
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    for mode in ('debug', 'release'):
        for level in ('off', 'info'):
            # One make invocation keeps the test hooks and the boot image in
            # sync: cargo re-fingerprints option_env! when the flags change.
            subprocess.run(['make', 'build', f'MODE={mode}', f'LOG={level}',
                            'FAT_TEST=1', 'DISK=1'],
                           cwd=root, check=True, stdout=subprocess.DEVNULL)
            app_dir = root / f'target/apps/{mode}'
            disk = app_dir / 'disk.img'
            hello = (app_dir / 'hello.elf').read_bytes()
            expected_size = len(hello)
            expected_sum = sum(hello[:4096])
            kernel = (root / f'target/kernel/{mode}-log{level}-test0/{TARGET}/{mode}/kernel')

            # The ext4 image is rebuilt first: the previous combo may have left
            # a deliberately corrupted image behind.
            subprocess.run(['make', 'disk', f'MODE={mode}', 'FS_TYPE=ext4'], cwd=root,
                           check=True, stdout=subprocess.DEVNULL)
            print(f'CHECK ext4 {mode} LOG={level}: mount + read', flush=True)

            def positive(text):
                return f'[fs] test read=4096 sum={expected_sum:#x}' in text

            text = run(args.qemu, kernel, disk, positive)
            assert '[fs] superblock: ext4' in text, 'the ext4 superblock was not detected'
            assert f'[fs] test size={expected_size}' in text, 'file size mismatch'
            assert f'[fs] mounted, {len(disk.read_bytes()) // 512} sectors' in text

            # Negative: a corrupted ext4 magic must fail detection - bounded.
            with disk.open('r+b') as image:
                image.seek(0x438)
                image.write(b'\x00\x00')
            print(f'CHECK ext4 {mode} LOG={level}: corrupt magic', flush=True)
            text = run(args.qemu, kernel, disk,
                       lambda t: '[fs] unknown filesystem superblock' in t)
            assert '[fs] mounted' not in text, 'mount accepted a corrupt magic'
    print('PASS: ext4 mount/read verified (lwext4, read-only); corrupt magic '
          'stays bounded.', flush=True)


if __name__ == '__main__':
    main()
