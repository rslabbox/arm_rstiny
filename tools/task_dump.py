#!/usr/bin/env python3
"""Live task-table dump through the QEMU gdbstub - the deadlock first
responder.

Boots the default image with a gdbstub, attaches, halts the vCPU and
prints every kernel task: slot, id, state, priority and the CSpace/VSpace
objects it is bound to. This is the same table `debug_dump_tasks` logs on
faults, available on demand for hangs that never fault (the case that ate
the respawn-livelock investigation).

    python3 tools/task_dump.py [--kernel <bootloader>] [--elf <kernel.elf>]
                               [--disk <img>] [--delay <seconds>]

Struct/field offsets are read from the kernel ELF's DWARF, so the dump
survives struct churn.
"""
import argparse
import os
import re
import socket
import select
import struct
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / 'tools'))
from check_kernel import Gdb  # noqa: E402

STATES = {0: 'created', 1: 'running', 2: 'suspended', 3: 'faulted', 4: 'ready',
          5: 'sleeping', 6: 'exited', 7: 'waiting', 8: 'blocked-send',
          9: 'blocked-recv', 10: 'blocked-reply', 11: 'blocked-fault'}


def scheduler_symbol(elf):
    out = subprocess.run(['rust-nm', '--defined-only', elf],
                         capture_output=True, text=True)
    for line in out.stdout.splitlines():
        if 'scheduler9SCHEDULER' in line:
            return int(line.split()[0], 16)
    raise SystemExit('SCHEDULER symbol not found in the kernel ELF')


def dwarf_layout(elf):
    from elftools.elf.elffile import ELFFile
    with open(elf, 'rb') as handle:
        dwarf = ELFFile(handle).get_dwarf_info()
    layout = {}
    for cu in dwarf.iter_CUs():
        for die in cu.iter_DIEs():
            if die.tag == 'DW_TAG_structure_type' and die.attributes.get('DW_AT_name'):
                name = die.attributes['DW_AT_name'].value.decode()
                if name in ('Scheduler', 'Task') and name not in layout:
                    if 'DW_AT_byte_size' not in die.attributes:
                        continue
                    size = die.attributes['DW_AT_byte_size'].value
                    members = {}
                    for child in die.iter_children():
                        if child.tag == 'DW_TAG_member':
                            key = child.attributes['DW_AT_name'].value.decode()
                            off = child.attributes.get('DW_AT_data_member_location')
                            members[key] = off.value if off else None
                    layout[name] = (size, members)
        if len(layout) == 2:
            break
    return layout


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--kernel',
                        default=str(ROOT / 'target/kernel/release-loginfo-test0/image/bootloader'))
    parser.add_argument('--elf',
                        default=str(ROOT / 'target/kernel/release-loginfo-test0/aarch64-unknown-none-softfloat/release/kernel'))
    parser.add_argument('--disk',
                        default=str(ROOT / 'target/apps/release/disk.img'))
    parser.add_argument('--gdb', default='/tmp/rstiny-taskdump.sock')
    parser.add_argument('--delay', type=float, default=6.0,
                        help='seconds to let the guest run before dumping')
    options = parser.parse_args()

    sched_addr = scheduler_symbol(options.elf)
    layout = dwarf_layout(options.elf)
    sched_size, sched_members = layout['Scheduler']
    task_size, task_members = layout['Task']

    qemu = subprocess.Popen([
        'qemu-system-aarch64', '-machine', 'virt,gic-version=3,virtualization=off',
        '-cpu', 'cortex-a72', '-smp', '1', '-m', '128M', '-display', 'none',
        '-monitor', 'none', '-nic', 'none',
        '-global', 'virtio-mmio.force-legacy=false',
        '-device', 'virtio-blk-device,drive=hd0',
        '-device', 'virtio-gpu-device,xres=640,yres=480',
        '-device', 'virtio-keyboard-device', '-device', 'virtio-mouse-device',
        '-drive', f'file={options.disk},if=none,format=raw,id=hd0,readonly=on',
        '-serial', 'stdio', '-kernel', str(Path(options.kernel)),
        '-gdb', f'unix:{options.gdb},server=on,wait=off',
    ], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL, cwd=str(ROOT))
    time.sleep(options.delay)

    # The scheduler address is a kernel VA; the physical base comes from the
    # boot banner ("kernel entry=VA; kernel paddr=PA").
    boot = b''
    while True:
        r, _, _ = select.select([qemu.stdout], [], [], 0.3)
        if not r:
            break
        chunk = os.read(qemu.stdout.fileno(), 4096)
        if not chunk:
            break
        boot += chunk
        match = re.search(rb'kernel entry=0x([0-9a-f]+); kernel paddr=0x([0-9a-f]+)', boot)
        if match:
            break
    if b'kernel entry=' not in boot:
        raise SystemExit('the boot banner never appeared')
    entry_va = int(re.search(rb'kernel entry=0x([0-9a-f]+)', boot).group(1), 16)
    entry_pa = int(re.search(rb'kernel paddr=0x([0-9a-f]+)', boot).group(1), 16)
    va_offset = entry_va - entry_pa
    sched_pa = sched_addr - va_offset
    print(f'scheduler {sched_addr:#x} (pa {sched_pa:#x})')

    try:
        gdb = Gdb(options.gdb, qemu)
        gdb.sock.settimeout(2.0)
        gdb.sock.sendall(b'\x03')  # halt the vCPU before touching memory
        time.sleep(0.5)
        # Drain whatever QEMU queued (banner, halt chatter, stop replies).
        try:
            while True:
                chunk = gdb.sock.recv(4096)
                if not chunk:
                    break
        except (TimeoutError, OSError):
            pass
        gdb.sock.settimeout(10.0)
        # Physical mode: the vCPU may be halted in a user task whose MMU
        # cannot see kernel VAs.
        gdb.command('Qqemu.PhyMemMode:1')

        def u64(addr):
            try:
                return int.from_bytes(gdb.memory(addr, 8), 'little')
            except AssertionError as error:
                print('u64 failed:', error, flush=True)
                raise

        def u32(addr):
            return int.from_bytes(gdb.memory(addr, 4), 'little')

        def pair(addr):
            return struct.unpack_from('<II', gdb.memory(addr, 8), 0)

        sched = sched_addr - va_offset  # physical base of the scheduler
        current_off = sched_members['current']
        stride = task_size
        count = (current_off - sched_members['tasks']) // task_size
        print(f'task table: stride {stride} ({count} slots), '
              f'current slot {u64(sched + current_off)}')
        for slot in range(count):
            base = sched + sched_members['tasks'] + slot * task_size
            tid = u64(base + task_members['id'])
            if tid == 0:
                continue
            state = u32(base + task_members['state'])
            cspace = pair(base + task_members['cspace'])
            vspace = pair(base + task_members['vspace'])
            print(f'  slot {slot:2d} id {tid:4d} {STATES.get(state, state):13s} '
                  f'cspace={cspace} vspace={vspace}')
    finally:
        qemu.terminate()
        try:
            qemu.wait(timeout=3)
        except Exception:
            qemu.kill()


if __name__ == '__main__':
    main()
