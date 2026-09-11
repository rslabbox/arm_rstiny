#!/usr/bin/env python3
"""Export QEMU's tree and generate build-time inputs for the fixed platform."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess


def run(args):
    return subprocess.check_output([str(a) for a in args], text=True)


def generate(output, qemu='qemu-system-aarch64'):
    output.mkdir(parents=True, exist_ok=True)
    machine = "virt,gic-version=3,virtualization=off"
    key = hashlib.sha256((run([qemu, '--version']) + run(['dtc', '--version'])
                          + machine).encode()
                         + Path(__file__).read_bytes()).hexdigest()
    products = ['qemu-arm-virt.dtb', 'qemu-arm-virt.dts', 'kernel.dts', 'kernel.dtb',
                'platform.rs', 'platform.json']
    stamp = output / 'platform.sha256'
    if stamp.exists() and stamp.read_text() == key and all((output / p).exists() for p in products):
        return
    raw = output / 'qemu-arm-virt.dtb'
    subprocess.run([qemu, '-machine', f'{machine},dumpdtb={raw}', '-cpu', 'cortex-a72',
                    '-smp', '1', '-m', '128M', '-display', 'none', '-nic', 'none'],
                   check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    dts = run(['dtc', '-q', '-I', 'dtb', '-O', 'dts', raw])
    (output / 'qemu-arm-virt.dts').write_text(dts)
    # Recompile the exported DTS to remove QEMU's unused DTB padding.
    merged = output / 'kernel.dts'
    merged.write_text(dts)
    dtb = output / 'kernel.dtb'
    subprocess.run(['dtc', '-q', '-I', 'dts', '-O', 'dtb', '-o', str(dtb), str(merged)], check=True)
    merged.write_text(run(['dtc', '-q', '-I', 'dtb', '-O', 'dts', dtb]))

    def get(node, prop, kind='x'):
        return run(['fdtget', '-t', kind, dtb, node, prop]).strip().split()

    def words(node, prop):
        return [int(x, 16) for x in get(node, prop)]

    def nodes(path='/'):
        yield path
        for child in run(['fdtget', '-l', dtb, path]).split():
            yield from nodes(path.rstrip('/') + '/' + child)

    all_nodes = list(nodes())
    properties = {n: run(['fdtget', '-p', dtb, n]).split() for n in all_nodes}
    compat = {n: get(n, 'compatible', 's') for n in all_nodes if 'compatible' in properties[n]}

    def match(name):
        found = [n for n in all_nodes if name in compat.get(n, [])]
        if len(found) != 1:
            raise ValueError(f'expected one {name} device, found {found}')
        return found[0]

    def regions(node):
        parent = node.rsplit('/', 1)[0] or '/'
        if words(parent, '#address-cells') != [2] or words(parent, '#size-cells') != [2]:
            raise ValueError(f'unsupported address format: {node}')
        raw = words(node, 'reg')
        if len(raw) % 4:
            raise ValueError(f'invalid reg: {node}')
        return [(raw[i] << 32 | raw[i + 1], raw[i + 2] << 32 | raw[i + 3])
                for i in range(0, len(raw), 4)]

    uart = match('arm,pl011')
    gic = match('arm,gic-v3')
    timer = match('arm,armv8-timer')
    psci = match('arm,psci-1.0')
    kernel_devices = [uart, gic, timer]
    loader_devices = [uart, timer, psci]
    method = get(psci, 'method', 's')[0]
    if method != 'hvc':
        raise ValueError('PSCI method does not match the selected QEMU machine')
    memory = [n for n in all_nodes if n.startswith('/memory@')]
    if len(memory) != 1 or regions(memory[0]) != [(0x40000000, 0x08000000)]:
        raise ValueError('linker/boot window requires QEMU 128 MiB RAM at 0x40000000')
    uart_base, uart_size = regions(uart)[0]
    (gicd, gicd_size), (gicr, gicr_size) = regions(gic)[:2]
    if (uart_base, uart_size, gicd, gicd_size, gicr) != (0x09000000, 0x1000, 0x08000000, 0x10000, 0x080a0000) or gicr_size < 0x20000:
        raise ValueError('unsupported QEMU MMIO layout')
    # The whole VirtIO MMIO window is published to user drivers as one device
    # Untyped: individual slots are 0x200 bytes and cannot satisfy the 4 KiB
    # minimum of an Untyped region (docs/disk-driver.md section 5.1).
    virtio = [node for node in all_nodes if 'virtio,mmio' in compat.get(node, [])]
    slots = sorted(regions(node)[0] for node in virtio)
    if not slots or len(slots) != len(virtio):
        raise ValueError(f'invalid virtio,mmio nodes: {virtio}')
    if any(size != 0x200 for _, size in slots):
        raise ValueError(f'unexpected virtio-mmio slot size: {slots}')
    if any(base != slots[0][0] + index * 0x200 for index, (base, _) in enumerate(slots)):
        raise ValueError(f'non-contiguous virtio-mmio slots: {slots}')
    virtio_base = slots[0][0]
    span = slots[-1][0] + slots[-1][1] - virtio_base
    virtio_size_log2 = max(12, (span - 1).bit_length())
    if virtio_base % (1 << virtio_size_log2):
        raise ValueError(f'virtio-mmio window is not aligned: {slots}')
    # The platform IRQ table (docs/irq.md section 3.1): one entry per
    # user-authorizable line, generated as (INTID, level, kind). Kind 0 is a
    # VirtIO MMIO slot line — the entries are ascending with the slot order and
    # must be contiguous SPIs sharing one trigger type, so a supervisor can map
    # device ordinal to line positionally. Kind 1 covers other devices' lines
    # (the PL011 today) and follows. Trigger/visibility is platform policy; the
    # timer PPI and every unmapped line stay kernel-owned.
    slot_irqs = []
    for node in [n for n in all_nodes if 'virtio,mmio' in compat.get(n, [])]:
        raw = words(node, 'interrupts')
        if len(raw) != 3 or raw[0] != 0:
            raise ValueError(f'expected a GIC SPI for {node}: {raw}')
        slot_irqs.append((regions(node)[0][0], raw[1], raw[2]))
    slot_irqs.sort()
    if any(number != slot_irqs[0][1] + index for index, (_, number, _) in enumerate(slot_irqs)):
        raise ValueError(f'virtio-mmio IRQs are not contiguous: {slot_irqs}')
    triggers = {flags & 15 for _, _, flags in slot_irqs}
    if len(triggers) != 1 or not triggers <= {1, 2, 4}:
        raise ValueError(f'unsupported virtio-mmio trigger configuration: {slot_irqs}')
    virtio_irq_level = triggers == {4}
    uart_raw = words(uart, 'interrupts')
    if len(uart_raw) != 3 or uart_raw[0] != 0 or uart_raw[2] & 15 not in (1, 2, 4):
        raise ValueError(f'expected a GIC SPI for {uart}: {uart_raw}')
    irq_lines = ([(number + 32, 1 if virtio_irq_level else 0, 0) for _, number, _ in slot_irqs] +
                 [(uart_raw[1] + 32, 1 if uart_raw[2] & 15 == 4 else 0, 1)])
    if len(set(intid for intid, _, _ in irq_lines)) != len(irq_lines):
        raise ValueError(f'duplicate platform IRQ lines: {irq_lines}')
    irq = words(timer, 'interrupts')[3:6]  # Non-secure physical timer.
    if len(irq) != 3 or irq[0] != 1 or irq[1] >= 16 or irq[2] & 15 != 4:
        raise ValueError('expected a level-triggered physical timer PPI')
    cpus = [n for n, c in compat.items() if 'arm,cortex-a72' in c]
    if len(cpus) != 1 or words(cpus[0], 'reg') != [0]:
        raise ValueError('only one Cortex-A72 CPU is supported')
    constants = dict(UART_BASE=uart_base, GICD_BASE=gicd, GICD_SIZE=gicd_size,
                     GICR_BASE=gicr, GICR_SIZE=0x20000, RAM_START=0x40000000, RAM_END=0x48000000,
                     VIRTIO_MMIO_BASE=virtio_base, VIRTIO_MMIO_SIZE=1 << virtio_size_log2)
    rust = '// Generated from QEMU kernel.dtb; do not edit.\n'
    # VIRTIO_MMIO_SIZE stays json-only: the window extent is device policy for
    # supervisors, and the kernel consumes the log2 form below.
    rust += ''.join(f'pub const {name}: usize = {value:#x};\n' for name, value in constants.items()
                    if name != 'VIRTIO_MMIO_SIZE')
    rust += f'pub const VIRTIO_MMIO_SIZE_LOG2: u8 = {virtio_size_log2};\n'
    # Per-line platform IRQ table: (INTID, level, kind); see docs/irq.md §3.1.
    rust += 'pub const IRQ_LINES: &[(u64, u64, u64)] = &['
    rust += ', '.join(f'({intid}, {level}, {kind})' for intid, level, kind in irq_lines)
    rust += '];\n'
    rust += f'pub const TIMER_IRQ: u32 = {irq[1] + 16};\npub const PSCI_SMC: bool = {str(method == "smc").lower()};\n'
    (output / 'platform.rs').write_text(rust)
    (output / 'platform.json').write_text(json.dumps(dict(constants, timer_irq=irq[1] + 16,
        irq_lines=[list(line) for line in irq_lines], virtio_slots=len(slots),
        psci_method=method, machine=machine,
        kernel_devices=kernel_devices, loader_devices=loader_devices), indent=2) + '\n')
    stamp.write_text(key)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--qemu', default='qemu-system-aarch64')
    args = parser.parse_args()
    generate(args.output.resolve(), args.qemu)
