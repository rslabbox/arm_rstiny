#!/usr/bin/env python3
"""Verify actual EL0 root execution, SVC restoration and hardware isolation."""
import argparse
from pathlib import Path
import struct
import subprocess
import tempfile
from check_kernel import Gdb, ROOT, TARGET, KERNEL_OFFSET, build, symbols, boot_image, kernel_output, translate, kernel_word

from elf_image import parse_elf, root_layout


def write(gdb, address, data):
    assert gdb.command(f'M{address:x},{len(data):x}:{data.hex()}') == 'OK'


def pages(gdb):
    result = {}
    mask = (1 << 40) - 4096
    def visit(table, level, prefix):
        for i, entry in enumerate(struct.unpack('<512Q', gdb.memory(table, 4096))):
            if not entry & 1:
                continue
            assert entry & 2
            va = prefix | (i << (39 - 9 * level))
            if level == 3:
                result[va] = (entry & mask, entry)
            else:
                visit(entry & mask, level + 1, va)
    assert gdb.command('Qqemu.PhyMemMode:1') == 'OK'
    try:
        visit(gdb.reg('ttbr0_el1') & mask, 0, 0)
    finally:
        assert gdb.command('Qqemu.PhyMemMode:0') == 'OK'
    return result


def check_layout(gdb, syms, elf, level, dtb, guard_protected=False):
    layout = root_layout(parse_elf(elf.read_bytes()), len(dtb))
    boot_info, ipc, extra = (layout[k] for k in ('boot_info', 'ipc', 'extra'))
    stack_bottom = symbols(elf)['__user_stack_bottom']
    mapping = pages(gdb)
    user = {va: p for va, p in mapping.items() if p[1] & (1 << 6)}
    data = elf.read_bytes()
    phoff = struct.unpack_from('<Q', data, 32)[0]
    phnum = struct.unpack_from('<H', data, 56)[0]
    expected = {boot_info: 4, ipc: 6}
    extra_size = len(dtb) + 16
    extra_pages = (extra_size + 4095) // 4096 * 4096
    expected.update({va: 4 for va in range(extra, extra + extra_pages, 4096)})
    for i in range(phnum):
        typ, flags, _, va, _, _, size, _ = struct.unpack_from('<IIQQQQQQ', data, phoff + i * 56)
        if typ == 1:
            for address in range(va, (va + size + 4095) // 4096 * 4096, 4096):
                expected[address] = flags
    if guard_protected:
        expected.pop(stack_bottom - 4096)
    assert user.keys() == expected.keys(), (user.keys() - expected.keys(), expected.keys() - user.keys())
    assert len({pa for pa, _ in user.values()}) == len(user), 'aliased user frames'
    for va, flags in expected.items():
        pa, attr = user[va]
        if va in (boot_info, ipc) or extra <= va < extra + extra_pages:
            assert translate(gdb, syms['__frames_start']) <= pa < translate(gdb, syms['__frames_end'] - 1) + 1
        else:
            offset = kernel_word(gdb, syms['LOADER_BOOT_INFO'] + 16)
            assert pa == va + offset, 'loader image was copied instead of adopted'
        assert attr & (1 << 53), 'user pages must be PXN'
        assert bool(attr & (1 << 7)) == (flags != 6)
        assert bool(attr & (1 << 54)) == (flags != 5)
    for va in (syms['_start'], syms['boot_stack'], 0x09000000):
        assert va not in mapping, 'kernel/MMIO present in user TTBR0'
    assert (stack_bottom - 4096 not in mapping) == guard_protected
    bi = struct.unpack('<8Q', gdb.memory(boot_info, 64))
    assert bi == (0x525354494e594249, 3, 64, 4096, int(level != 'off'), ipc, extra, extra_size)
    assert gdb.memory(boot_info + 64, 4096 - 64) == bytes(4096 - 64)
    assert gdb.memory(extra, 16) == struct.pack('<QQ', 6, extra_size)
    assert gdb.memory(extra + 16, len(dtb)) == dtb, 'DTB changed during BootInfo transfer'
    assert gdb.memory(extra + extra_size, extra_pages - extra_size) == bytes(extra_pages - extra_size)


def run(qemu, kernel, user, level, scenario):
    ks, us = symbols(kernel), symbols(user)
    dtb = (boot_image(kernel).parent / 'kernel.dtb').read_bytes()
    layout = root_layout(parse_elf(user.read_bytes()), len(dtb))
    boot_info, ipc, extra = (layout[k] for k in ('boot_info', 'ipc', 'extra'))
    stack_bottom, stack_top = us['__user_stack_bottom'], us['__user_stack_top']
    assert stack_top - stack_bottom == 32 * 1024, 'root stack_size attribute ignored'
    assert stack_bottom % 4096 == 0
    with tempfile.TemporaryDirectory(prefix='rstiny-fatboot-') as temp:
        temp = Path(temp)
        output, errors = temp / 'serial.log', temp / 'qemu.log'
        with errors.open('wb') as err:
            proc = subprocess.Popen([
                qemu, '-machine', "virt,gic-version=3,virtualization=off",
                '-cpu', 'cortex-a72', '-smp', '1', '-m', '128M', '-display', 'none',
                '-monitor', 'none', '-serial', f'file:{output}', '-nic', 'none',
                '-kernel', str(boot_image(kernel)), '-S', '-gdb', f'unix:{temp / "gdb"},server=on,wait=off',
            ], stderr=err, stdout=subprocess.DEVNULL)
            gdb = None
            try:
                gdb = Gdb(temp / 'gdb', proc)
                gdb.run_to(us['_start'])
                assert gdb.reg('cpsr') & 15 == 0, 'not EL0t'
                assert gdb.reg('sp') == 0 and gdb.reg('x0') == boot_info
                assert all(gdb.reg(f'x{i}') == 0 for i in range(1, 31)), 'stale initial GPR'
                # The user runtime, rather than the kernel, installs its ELF-owned stack.
                assert gdb.command('s').startswith(('T05', 'S05'))
                assert gdb.command('s').startswith(('T05', 'S05'))
                assert gdb.reg('sp') == stack_top
                gdb.write_reg('pc', us['_start'])
                mapping = pages(gdb)
                def user_word(va):
                    assert gdb.command('Qqemu.PhyMemMode:1') == 'OK'
                    try:
                        return gdb.word(mapping[va & -4096][0] + (va & 4095))
                    finally:
                        assert gdb.command('Qqemu.PhyMemMode:0') == 'OK'
                if scenario in ('normal', 'ignored-dtb'):
                    check_layout(gdb, ks, user, level, (boot_image(kernel).parent / 'kernel.dtb').read_bytes())
                    gdb.run_to(us['__rstiny_root_start'])
                    check_layout(gdb, ks, user, level, dtb, guard_protected=True)
                    mapping = pages(gdb)
                    assert user_word(us['RESULT']) == 0
                    if scenario == 'ignored-dtb':
                        write(gdb, extra + 16, bytes(4))
                    hello = user.parents[2] / 'hello.elf'
                    hello_data = hello.read_bytes()
                    hello_image = parse_elf(hello_data)
                    entry = hello_image['entry']
                    child_stack_bottom = hello_image['end'] + 4096
                    child_stack_top = child_stack_bottom + 16384
                    gdb.run_to(entry)
                    assert gdb.reg('cpsr') & 15 == 0
                    assert gdb.reg('sp') == child_stack_top and gdb.reg('x0') == 0
                    child_pages = pages(gdb)
                    assert not {pa for pa, _ in mapping.values()} & {pa for pa, _ in child_pages.values()}, 'root and child share physical frames'
                    assert child_stack_bottom - 4096 not in child_pages, 'missing child stack guard'
                    phoff = struct.unpack_from('<Q', hello_data, 32)[0]
                    phnum = struct.unpack_from('<H', hello_data, 56)[0]
                    expected = set(range(child_stack_bottom, child_stack_top, 4096))
                    for i in range(phnum):
                        typ, flags, offset, va, _, filesz, memsz, _ = struct.unpack_from('<IIQQQQQQ', hello_data, phoff + i * 56)
                        if typ != 1 or memsz == 0:
                            continue
                        for address in range(va, (va + memsz + 4095) & -4096, 4096):
                            expected.add(address)
                            attr = child_pages[address][1]
                            assert attr & (1 << 6) and attr & (1 << 53)
                            assert bool(attr & (1 << 7)) == (flags != 6)
                            assert bool(attr & (1 << 54)) == (flags != 5)
                        assert gdb.memory(va, filesz) == hello_data[offset:offset + filesz]
                        assert gdb.memory(va + filesz, memsz - filesz) == bytes(memsz - filesz)
                    assert child_pages.keys() == expected
                    assert gdb.memory(child_stack_bottom, 16384) == bytes(16384)
                    for _ in range(10):
                        gdb.run_to(ks['root_idle'])
                        if gdb.reg('x0') == 2:
                            break
                        assert gdb.reg('x0') == 5, 'unexpected root stop'
                        assert gdb.command('s').startswith(('T05', 'S05'))
                    assert gdb.reg('x0') == 2
                    assert user_word(us['RESULT']) == 1
                    assert user_word(us['BOOTINFO_ADDRESS']) == boot_info
                    assert user_word(ipc) == 0
                elif scenario in ('invalid-bootinfo', 'invalid-extra', 'invalid-hello', 'invalid-hello-entry'):
                    # Corrupt the version through the debugger, before the
                    # runtime constructs its safe BootInfo view.
                    if scenario == 'invalid-bootinfo':
                        write(gdb, boot_info + 8, struct.pack('<Q', 999))
                    elif scenario == 'invalid-extra':
                        write(gdb, extra + 8, struct.pack('<Q', 0))
                    elif scenario == 'invalid-hello':
                        write(gdb, us['__hello_start'], bytes(4))
                    else:
                        write(gdb, us['__hello_start'] + 24, struct.pack('<Q', 0))
                    gdb.run_to(ks['root_idle'])
                    assert gdb.reg('x0') == 2
                    assert user_word(us['RESULT']) == 0
                    assert user_word(us['BOOTINFO_ADDRESS']) == (boot_info if scenario.startswith('invalid-hello') else 0)
                    assert proc.poll() is None
                elif scenario == 'svc-registers':
                    # Patch a scratch instruction at the user entry through the
                    # debugger; guest writes to this RX page are tested below.
                    write(gdb, us['_start'], struct.pack('<I', 0xd4000001)) # svc #0
                    for number, argument, result in [(0, 17, 0), (20, 0, 1), (999, 0, 1), (2**64 - 1, 0, 1), (1, 256, 2)]:
                        values = {f'x{i}': 0x12340000 + i for i in range(31)}
                        values['x0'], values['x8'] = argument, number
                        for reg, value in values.items():
                            gdb.write_reg(reg, value)
                        gdb.write_reg('pc', us['_start'])
                        gdb.run_to(us['_start'] + 4)
                        assert gdb.reg('cpsr') & 15 == 0
                        assert gdb.reg('sp') == stack_top
                        for reg, value in values.items():
                            assert gdb.reg(reg) == (result if reg == 'x0' else value), reg
                    if level == 'off':
                        gdb.write_reg('x8', 1)
                        gdb.write_reg('x0', 65)
                        gdb.write_reg('pc', us['_start'])
                        gdb.run_to(us['_start'] + 4)
                        assert gdb.reg('x0') == 1
                else:
                    # Injections only change instructions/registers through the
                    # host debugger. Access checks are executed by the EL0 CPU.
                    if scenario == 'guard-read':
                        gdb.run_to(us['__rstiny_root_start'])
                        assert stack_bottom - 4096 not in pages(gdb)
                        gdb.write_reg('pc', us['_start'])
                    target, instruction, ec, permission = {
                        'kernel-read': (ks['_start'], 0xf9400001, 0x24, True),
                        'uart-read': (0x09000000, 0xf9400001, 0x24, False),
                        'text-write': (us['_start'], 0xf9000001, 0x24, True),
                        'bootinfo-write': (boot_info, 0xf9000001, 0x24, True),
                        'dtb-write': (extra + 16, 0xf9000001, 0x24, True),
                        'guard-read': (stack_bottom - 4096, 0xf9400001, 0x24, False),
                        'null-read': (0, 0xf9400001, 0x24, False),
                        'stack-execute': (stack_bottom, 0xd61f0000, 0x20, True),
                    }[scenario]
                    write(gdb, us['_start'], struct.pack('<I', instruction))
                    gdb.write_reg('x0', target)
                    gdb.run_to(ks['root_idle'])
                    assert gdb.reg('x0') == 3, 'faulting root task not stopped'
                    record = struct.unpack('<38Q', gdb.memory(ks['LAST_FAULT'], 304))
                    assert record[:2] == (0, 2), 'not a lower-EL synchronous fault'
                    assert record[2] >> 26 == ec and record[3] == target
                    assert record[2] & 63 == 15 if permission else 4 <= record[2] & 63 <= 7
                    assert record[37] & 15 == 0
                    assert proc.poll() is None, 'user fault shut down the kernel'
                text = kernel_output(output.read_bytes())
                if level == 'off':
                    assert not text, text
                elif scenario in ('normal', 'ignored-dtb'):
                    assert b'[fatboot] loading hello.elf' in text
                    assert b'[hello] Hello, world!' in text
                    assert b'[fatboot] hello.elf exited successfully' in text
                elif scenario in ('invalid-bootinfo', 'invalid-extra', 'invalid-hello', 'invalid-hello-entry'):
                    assert b'[user panic]' in text
                    assert (b'[fatboot] loading hello.elf' in text) == (scenario.startswith('invalid-hello'))
            except Exception:
                print(errors.read_text(), output.read_text(errors='replace') if output.exists() else '', flush=True)
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
        for level in ('off', 'info'):
            kernel = build(mode, level, False)
            user = ROOT / f'target/apps/{mode}/{TARGET}/{mode}/fatboot'
            print(f'CHECK fatboot {mode} LOG={level}', flush=True)
            run(args.qemu, kernel, user, level, 'normal')
            for scenario in ('ignored-dtb', 'invalid-bootinfo', 'invalid-extra', 'invalid-hello', 'invalid-hello-entry', 'svc-registers', 'kernel-read', 'uart-read', 'text-write',
                             'bootinfo-write', 'dtb-write', 'guard-read', 'null-read', 'stack-execute'):
                print(f'  {scenario}', flush=True)
                run(args.qemu, kernel, user, level, scenario)
    print('PASS: EL0 fatboot; BootInfo; SVC register preservation; user fault isolation.', flush=True)


if __name__ == '__main__':
    main()
