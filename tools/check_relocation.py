#!/usr/bin/env python3
"""Boot the same high-linked kernel at different RAM locations under QEMU."""
import argparse
import hashlib
import subprocess
import tempfile
from pathlib import Path
from check_kernel import Gdb, build, boot_image, symbols, translate, check_handoff, check_layout
from check_fatboot import run as run_root, ROOT, TARGET
from check_tasks import run as run_tasks
from elf_image import parse_elf
from check_bootloader import run as reject


def check_mapping(qemu, kernel, minimum):
    syms = symbols(kernel)
    loader = symbols(boot_image(kernel))
    with tempfile.TemporaryDirectory(prefix='rstiny-relocation-') as tmp:
        tmp = Path(tmp)
        proc = subprocess.Popen([
            qemu, '-machine', 'virt,gic-version=3,virtualization=off', '-cpu', 'cortex-a72',
            '-smp', '1', '-m', '128M', '-display', 'none', '-monitor', 'none',
            '-serial', f'file:{tmp / "serial"}', '-nic', 'none', '-kernel', str(boot_image(kernel)),
            '-S', '-gdb', f'unix:{tmp / "gdb"},server=on,wait=off',
        ], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        gdb = None
        try:
            gdb = Gdb(tmp / 'gdb', proc)
            gdb.run_to(syms['_start'])
            physical = translate(gdb, syms['skernel'])
            assert physical >= minimum and physical != 0x40200000
            end = gdb.reg('x1') + 4096
            assert end <= loader['__loader_start'] or physical >= loader['__loader_end'], 'loader overlap'
            check_handoff(gdb, kernel)
            gdb.run_to(syms['start_root'])
            assert gdb.word(syms['LOADER_BOOT_INFO'] + 48) == physical, 'kernel did not discover its PA'
            check_layout(gdb, syms)
            print(f'  VA {syms["skernel"]:#x} -> PA {physical:#x}', flush=True)
        except Exception:
            print((tmp / 'serial').read_text(errors='replace'), flush=True)
            raise
        finally:
            if gdb:
                gdb.sock.close()
            proc.terminate()
            proc.wait(timeout=5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--qemu', default='qemu-system-aarch64')
    args = parser.parse_args()
    for mode in ('debug', 'release'):
        original = build(mode, 'info', False, 0)
        digest = hashlib.sha256(original.read_bytes()).digest()
        for minimum in (0x41000000, 0x43e00000):
            kernel = build(mode, 'info', False, minimum)
            assert hashlib.sha256(kernel.read_bytes()).digest() == digest, 'kernel rebuilt for physical placement'
            print(f'CHECK relocation {mode} minimum={minimum:#x}', flush=True)
            check_mapping(args.qemu, kernel, minimum)
            user = ROOT / f'target/apps/{mode}/{TARGET}/{mode}/fatboot'
            run_root(args.qemu, kernel, user, 'info', 'normal')
            if minimum == 0x43e00000:
                run_tasks(args.qemu, kernel)
        # Root VAs belong to its ELF; changing them must not rebuild the kernel.
        for root_base in (0x800000, 0x3000000):
            try:
                kernel = build(mode, 'info', False, 0, root_base)
                assert hashlib.sha256(kernel.read_bytes()).digest() == digest, 'kernel depends on root layout'
                print(f'CHECK root layout {mode} base={root_base:#x}', flush=True)
                user = ROOT / f'target/apps/{mode}/{TARGET}/{mode}/fatboot'
                assert parse_elf(user.read_bytes())['start'] == root_base
                run_root(args.qemu, kernel, user, 'info', 'normal')
                run_tasks(args.qemu, kernel)
            finally:
                build(mode, 'info', False, 0)
        # An impossible placement must fail before modifying destination RAM.
        kernel = build(mode, 'info', False, 0x48000000)
        reject(args.qemu, boot_image(kernel), 0, b'070701')
        build(mode, 'info', False, 0)
    print('PASS: identical kernel ELF supports physical relocation and different root VAs; mappings, hello, tasks, exhaustion.', flush=True)


if __name__ == '__main__':
    main()
