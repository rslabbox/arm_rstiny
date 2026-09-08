#!/usr/bin/env python3
"""Reject malformed boot archives in the real Rust bootloader before loading RAM."""
import argparse
from pathlib import Path
import struct
import subprocess
import tempfile

from check_kernel import Gdb, boot_image, build, symbols


def archive_entries(data):
    entries = {}
    cursor = 0
    while True:
        assert data[cursor:cursor + 6] == b'070701'
        size = int(data[cursor + 54:cursor + 62], 16)
        name_size = int(data[cursor + 94:cursor + 102], 16)
        start = cursor + 110
        name = data[start:start + name_size - 1].decode()
        payload = (start + name_size + 3) & -4
        if name == 'TRAILER!!!':
            return entries
        entries[name] = (cursor, payload, size)
        cursor = (payload + size + 3) & -4


def write(gdb, address, data):
    assert gdb.command(f'M{address:x},{len(data):x}:{data.hex()}') == 'OK'


def run(qemu, image, offset, replacement):
    syms = symbols(image)
    with tempfile.TemporaryDirectory(prefix='rstiny-bootloader-') as tmp:
        tmp = Path(tmp)
        serial = tmp / 'serial'
        with (tmp / 'errors').open('wb') as errors:
            proc = subprocess.Popen([
                qemu, '-machine', 'virt,gic-version=3,virtualization=off', '-cpu', 'cortex-a72',
                '-smp', '1', '-m', '128M', '-display', 'none', '-monitor', 'none',
                '-serial', f'file:{serial}', '-nic', 'none', '-kernel', str(image),
                '-S', '-gdb', f'unix:{tmp / "gdb"},server=on,wait=off',
            ], stderr=errors, stdout=subprocess.DEVNULL)
            gdb = None
            try:
                gdb = Gdb(tmp / 'gdb', proc)
                write(gdb, syms['__archive_start'] + offset, replacement)
                sentinel = b'UNTOUCHED-KERNEL!'
                write(gdb, 0x40200000, sentinel)
                # A malformed input must halt, never enter the high-address kernel.
                assert gdb.command('Z1,ffff000040200000,4') == 'OK'
                gdb.run_to(syms['bootloader_halt'])
                assert gdb.memory(0x40200000, len(sentinel)) == sentinel, 'partial load before rejection'
                assert not gdb.reg('sctlr_el1') & 1, 'MMU enabled for rejected input'
                output = serial.read_bytes()
                assert b'bootloader: error:' in output, output
                assert b'Enabling MMU and jumping to entry point' not in output
            except Exception:
                print((tmp / 'errors').read_text(), serial.read_text(errors='replace') if serial.exists() else '')
                raise
            finally:
                if gdb:
                    gdb.sock.close()
                proc.terminate()
                try:
                    proc.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    proc.kill()
                    proc.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--qemu', default='qemu-system-aarch64')
    args = parser.parse_args()
    for mode in ('debug', 'release'):
        image = boot_image(build(mode, 'info', False))
        data = (image.parent / 'archive.cpio').read_bytes()
        entries = archive_entries(data)
        _, kernel, _ = entries['kernel.elf']
        _, root, _ = entries['rootserver']
        _, dtb, _ = entries['kernel.dtb']
        phoff = struct.unpack_from('<Q', data, kernel + 32)[0]
        cases = {
            'cpio-magic': (0, b'BADBAD'),
            'cpio-extent': (54, b'ffffffff'),
            'kernel-machine': (kernel + 18, struct.pack('<H', 62)),
            'kernel-entry': (kernel + 24, struct.pack('<Q', 0)),
            'kernel-physical': (kernel + phoff + 24, struct.pack('<Q', 0x44000000)),
            'root-entry': (root + 24, struct.pack('<Q', 0x600000)),
            'dtb-short': (dtb + 4, struct.pack('>I', 39)),
            'dtb-extent': (dtb + 4, struct.pack('>I', 0xffffffff)),
        }
        for name, (offset, replacement) in cases.items():
            print(f'CHECK bootloader {mode} {name}', flush=True)
            run(args.qemu, image, offset, replacement)
    print('PASS: Rust bootloader rejects malformed CPIO/ELF/DTB before writing destination RAM.', flush=True)


if __name__ == '__main__':
    main()
