#!/usr/bin/env python3
"""Export QEMU's tree, apply the platform overlay and generate build-time inputs."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]
OVERLAY = ROOT / 'kernel/plat/qemu-arm-virt/overlay.dts'


def run(args):
    return subprocess.check_output([str(a) for a in args], text=True)


def generate(output, qemu='qemu-system-aarch64'):
    output.mkdir(parents=True, exist_ok=True)
    machine = "virt,gic-version=3,virtualization=off"
    key = hashlib.sha256((run([qemu, '--version']) + run(['dtc', '--version'])
                          + machine).encode() + OVERLAY.read_bytes()
                         + Path(__file__).read_bytes()).hexdigest()
    products = ['qemu-arm-virt.dtb', 'qemu-arm-virt.dts', 'kernel.dts', 'kernel.dtb',
                'platform.rs', 'platform.json', 'devices_gen.h', 'platform_info.h']
    stamp = output / 'platform.sha256'
    if stamp.exists() and stamp.read_text() == key and all((output / p).exists() for p in products):
        return
    raw = output / 'qemu-arm-virt.dtb'
    subprocess.run([qemu, '-machine', f'{machine},dumpdtb={raw}', '-cpu', 'cortex-a72',
                    '-smp', '1', '-m', '128M', '-display', 'none', '-nic', 'none'],
                   check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    dts = run(['dtc', '-q', '-I', 'dtb', '-O', 'dts', raw])
    (output / 'qemu-arm-virt.dts').write_text(dts)
    # As in seL4's DTS list, dtc merges the additional root-node definition.
    merged = output / 'kernel.dts'
    merged.write_text(dts + '\n' + OVERLAY.read_text())
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
    kernel_devices = get('/chosen', 'seL4,kernel-devices', 's')
    loader_devices = get('/chosen', 'seL4,elfloader-devices', 's')

    def match(devices, name):
        found = [n for n in devices if name in compat.get(n, [])]
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

    uart = match(kernel_devices, 'arm,pl011')
    gic = match(kernel_devices, 'arm,gic-v3')
    timer = match(kernel_devices, 'arm,armv8-timer')
    psci = match(loader_devices, 'arm,psci-1.0')
    if set(kernel_devices) != {uart, gic, timer} or set(loader_devices) != {uart, timer, psci}:
        raise ValueError('unsupported platform device selection')
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
    irq = words(timer, 'interrupts')[3:6]  # Non-secure physical timer.
    if len(irq) != 3 or irq[0] != 1 or irq[1] >= 16 or irq[2] & 15 != 4:
        raise ValueError('expected a level-triggered physical timer PPI')
    cpus = [n for n, c in compat.items() if 'arm,cortex-a72' in c]
    if len(cpus) != 1 or words(cpus[0], 'reg') != [0]:
        raise ValueError('only one Cortex-A72 CPU is supported')
    constants = dict(UART_BASE=uart_base, GICD_BASE=gicd, GICD_SIZE=gicd_size,
                     GICR_BASE=gicr, GICR_SIZE=0x20000, RAM_START=0x40000000, RAM_END=0x48000000)
    rust = '// Generated from the merged kernel.dtb; do not edit.\n'
    rust += ''.join(f'pub const {name}: usize = {value:#x};\n' for name, value in constants.items())
    rust += f'pub const TIMER_IRQ: u32 = {irq[1] + 16};\npub const PSCI_SMC: bool = {str(method == "smc").lower()};\n'
    (output / 'platform.rs').write_text(rust)
    (output / 'platform.json').write_text(json.dumps(dict(constants, timer_irq=irq[1] + 16,
        psci_method=method, machine=machine, kernel_devices=kernel_devices,
        loader_devices=loader_devices), indent=2) + '\n')
    entries = []
    for node in loader_devices:
        base = regions(node)[0][0] if 'reg' in properties[node] else 0
        entries.append(f'    {{ .compat = "{compat[node][0]}", .region_bases = {{ (void *){base:#x} }} }},')
    cpu_method = get(cpus[0], 'enable-method', 's')[0] if 'enable-method' in properties[cpus[0]] else None
    if cpu_method not in (None, 'psci'):
        raise ValueError('unsupported CPU enable method')
    cpu_method_c = json.dumps(cpu_method) if cpu_method else 'NULL'
    extra = (1 if method == 'smc' else 2) if cpu_method else 0
    (output / 'devices_gen.h').write_text('''/* Generated from kernel.dtb. */
#pragma once
#include <types.h>
#define MAX_NUM_REGIONS 1
struct elfloader_driver;
struct elfloader_device {
    const char *compat;
    volatile void *region_bases[MAX_NUM_REGIONS];
    struct elfloader_driver *drv;
};
struct elfloader_cpu {
    const char *compat;
    const char *enable_method;
    word_t cpu_id;
    word_t extra_data;
};
#ifdef DRIVER_COMMON
struct elfloader_device elfloader_devices[] = {
''' + '\n'.join(entries) + '''
};
struct elfloader_cpu elfloader_cpus[] = {
''' + f'    {{ .compat = "arm,cortex-a72", .enable_method = {cpu_method_c}, .cpu_id = 0, .extra_data = {extra} }},\n' + '''
    { .compat = NULL },
};
#else
extern struct elfloader_device elfloader_devices[];
extern struct elfloader_cpu elfloader_cpus[];
#endif
''')
    (output / 'platform_info.h').write_text('''/* Generated from kernel.dtb. */
int num_memory_regions = 1;
struct memory_region { size_t start; size_t end; } memory_region[1] = {
    { .start = 0x40000000, .end = 0x48000000 },
};
''')
    stamp.write_text(key)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--qemu', default='qemu-system-aarch64')
    args = parser.parse_args()
    generate(args.output.resolve(), args.qemu)
