#!/usr/bin/env python3
"""Verify boot-partitioned Untyped: enumeration, retype, Revoke and device policy."""
import argparse
from pathlib import Path
import struct
import subprocess
import tempfile
from check_kernel import Gdb, build, boot_image
from elf_image import parse_elf, root_layout
from abi_client import Client

UNTYPED = 32


def read_bootinfo(gdb, boot_info):
    """Return (untyped_start, descriptors) from the v6 BootInfo extension."""
    bi = struct.unpack('<16Q', gdb.memory(boot_info, 128))
    assert bi[0] == 0x525354494e594249, 'bad BootInfo magic'
    assert bi[1] == 6 and bi[2] == 128 and bi[3] == 4096, bi[:4]
    assert bi[10:16] == (0,) * 6, 'reserved BootInfo words must be zero'
    extra, untyped_start, untyped_count = bi[6], bi[8], bi[9]
    assert untyped_start == UNTYPED
    fdt_len = struct.unpack('<Q', gdb.memory(extra + 8, 8))[0]
    record = extra + (fdt_len + 7) // 8 * 8
    assert struct.unpack('<Q', gdb.memory(record, 8))[0] == 7
    payload = record + 16
    descriptors = [
        struct.unpack('<4Q', gdb.memory(payload + index * 32, 32))
        for index in range(untyped_count)
    ]
    return untyped_start, descriptors


def run(qemu, kernel):
    directory = boot_image(kernel).parent
    image = parse_elf((directory / 'userboot').read_bytes())
    layout = root_layout(image, (directory / 'kernel.dtb').stat().st_size)
    entry, ipc, boot_info = image['entry'], layout['ipc'], layout['boot_info']
    with tempfile.TemporaryDirectory(prefix='rstiny-untyped-') as temporary:
        tmp = Path(temporary)
        proc = subprocess.Popen([
            qemu, '-machine', 'virt,gic-version=3,virtualization=off', '-cpu', 'cortex-a72',
            '-smp', '1', '-m', '128M', '-display', 'none', '-monitor', 'none', '-nic', 'none',
            '-serial', f'file:{tmp / "serial"}', '-kernel', str(boot_image(kernel)),
            '-S', '-gdb', f'unix:{tmp / "gdb"},server=on,wait=off',
        ], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        gdb = None
        try:
            gdb = Gdb(tmp / 'gdb', proc)
            gdb.run_to(entry)
            client = Client(gdb, entry, ipc)
            untyped_start, descriptors = read_bootinfo(gdb, boot_info)
            assert descriptors, 'no Untyped regions published'
            # GIC/timer are kernel-reserved; the UART and the VirtIO MMIO
            # window are the device regions, published in ascending physical
            # order regardless of size (docs/disk-driver.md section 5.1).
            devices = [d for d in descriptors if d[2] == 1]
            assert all(d[0] != 0x08000000 and d[0] != 0x080A0000 for d in descriptors)
            assert [(d[0], d[1]) for d in devices] == [(0x09000000, 12), (0x0a000000, 14)], devices
            normal = [i for i, d in enumerate(descriptors) if d[2] == 0]
            assert normal, 'no ordinary Untyped regions'
            # Retype a frame from the largest ordinary region.
            index = max(normal, key=lambda i: descriptors[i][1])
            cap = untyped_start + index
            before = client.runtime('available')
            client.call(cap, 1, [7, 0, 0, 0, 200, 1], [2])
            assert client.runtime('available') == before - 1
            # Device Untyped may only become a device frame.
            device_cap = untyped_start + descriptors.index(next(d for d in devices))
            client.call(device_cap, 1, [7, 0, 0, 0, 201, 1], [2])
            client.call(device_cap, 1, [9, 0, 0, 0, 202, 1], [2], status=1)
            # A device frame maps Device/NX; executable mapping is rejected.
            # The target VA needs an L3 table first.
            client.call(cap, 1, [9, 0, 0, 0, 203, 1], [2])
            client.call(203, 38, [0x07000000, 1], [3])
            client.call(201, 40, [0x07000000, 3, 4], [3])
            client.call(201, 41)
            client.call(201, 40, [0x07000000, 3, 0], [3], status=1)
            # Revoke is region-granular: it finalises children and rewinds the
            # watermark, so the ordinary region returns to its boot state.
            client.call(2, 17, [cap, 64])
            assert client.runtime('available') == before
            client.call(200, 46, status=6)
            assert proc.poll() is None
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
        for level in ('off', 'info'):
            kernel = build(mode, level, False)
            print(f'CHECK Untyped {mode} LOG={level}', flush=True)
            run(args.qemu, kernel)
    print('PASS: BootInfo v6 enumeration; Untyped retype/accounting; device policy; region-granular Revoke.', flush=True)


if __name__ == '__main__':
    main()
