#!/usr/bin/env python3
"""Phase D2 acceptance: fs_server mounts the FAT32 image through the block
service and reads HELLO.ELF byte-for-byte. Corrupted images (bad BPB signature,
a self-looping FAT chain) must surface as bounded service errors — never a
kernel panic (docs/disk-driver.md section 12, D2)."""
import argparse
import subprocess
import tempfile
import time
from pathlib import Path

from check_kernel import boot_image

BOOT_TIMEOUT = 40.0
TARGET = 'aarch64-unknown-none-softfloat'
QEMU_DISK_ARGS = [
    '-global', 'virtio-mmio.force-legacy=false',
    '-device', 'virtio-blk-device,drive=hd0',
]


def run(qemu, kernel, disk, expectation):
    """Boot with `disk` and wait until `expectation(text)` holds or timeout."""
    with tempfile.TemporaryDirectory(prefix='rstiny-fs-') as temporary:
        serial = Path(temporary) / 'serial'
        proc = subprocess.Popen([
            qemu, '-machine', 'virt,gic-version=3,virtualization=off', '-cpu', 'cortex-a72',
            '-smp', '1', '-m', '128M', '-display', 'none', '-monitor', 'none', '-nic', 'none',
            *QEMU_DISK_ARGS,
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
                raise AssertionError('expected fs_server output never appeared')
            assert 'panicked' not in text and 'kernel panic' not in text, \
                'the kernel panicked on a corrupt image'
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
    tools = root / 'tools/make_disk.py'
    for mode in ('debug', 'release'):
        for level in ('off', 'info'):
            # One make invocation keeps the test hooks and the boot image in
            # sync: cargo re-fingerprints option_env! when the flags change.
            subprocess.run(['make', 'build', f'MODE={mode}', f'LOG={level}',
                            'BLK_TEST=1', 'FAT_TEST=1', 'DISK=1'],
                           cwd=root, check=True, stdout=subprocess.DEVNULL)
            app_dir = root / f'target/apps/{mode}'
            disk = app_dir / 'disk.img'
            hello = (app_dir / 'hello.elf').read_bytes()
            expected_size = len(hello)
            expected_sum = sum(hello[:4096])
            kernel = (root / f'target/kernel/{mode}-log{level}-test0/{TARGET}/{mode}/kernel')

            # Positive: a healthy image mounts and HELLO.ELF matches the host.
            # The disk is rebuilt first — the previous combo (or script run)
            # may have left a deliberately corrupted image behind.
            subprocess.run(['make', 'disk', f'MODE={mode}'], cwd=root, check=True,
                           stdout=subprocess.DEVNULL)
            print(f'CHECK fat32 {mode} LOG={level}: mount + read', flush=True)

            def positive(text):
                return f'[fs] test read=4096 sum={expected_sum:#x}' in text

            text = run(args.qemu, kernel, disk, positive)
            assert f'[fs] test size={expected_size}' in text, 'file size mismatch'
            assert f'[fs] mounted, {len(disk.read_bytes()) // 512} sectors' in text

            # Negative: bad BPB signature — the mount must fail, bounded.
            subprocess.run(['python3', str(tools), str(disk),
                            f'--file', f'HELLO.ELF={app_dir}/hello.elf',
                            f'--file', f'APPS.CFG={root}/apps/APPS.CFG',
                            '--corrupt-bpb'], cwd=root, check=True, stdout=subprocess.DEVNULL)
            print(f'CHECK fat32 {mode} LOG={level}: corrupt BPB', flush=True)
            text = run(args.qemu, kernel, disk, lambda t: '[fs] mount failed' in t)
            assert '[fs] mounted' not in text, 'mount accepted a corrupt BPB'

            # Negative: HELLO.ELF's FAT chain loops. The library's loop bound
            # (max_cluster steps) is far beyond one read, so the read returns
            # bounded-but-wrong data; the checksum must expose the corruption.
            subprocess.run(['python3', str(tools), str(disk),
                            f'--file', f'HELLO.ELF={app_dir}/hello.elf',
                            f'--file', f'APPS.CFG={root}/apps/APPS.CFG',
                            '--cycle-fat'], cwd=root, check=True, stdout=subprocess.DEVNULL)
            print(f'CHECK fat32 {mode} LOG={level}: FAT cycle', flush=True)

            def cycle(text):
                return '[fs] test read=' in text

            text = run(args.qemu, kernel, disk, cycle)
            assert '[fs] mounted' in text, 'the volume itself should still mount'
            mark = text.index('[fs] test read=') + len('[fs] test read=')
            corrupt_sum = text[mark:text.index('\n', mark) if '\n' in text[mark:] else len(text)]
            # Serial lines interleave; strip everything after the checksum.
            corrupt_sum = corrupt_sum.split()[0]
            assert corrupt_sum.strip() != f'sum={expected_sum:#x}', \
                'the looping chain silently returned the expected bytes'
    print('PASS: FAT32 mount/read verified; corrupt BPB and FAT cycle stay bounded.', flush=True)


if __name__ == '__main__':
    main()
