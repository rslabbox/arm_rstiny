#!/usr/bin/env python3
"""Verify actual EL0 root execution, SVC restoration and hardware isolation."""
import argparse
from pathlib import Path
import struct
import subprocess
import tempfile
from check_kernel import Gdb, ROOT, TARGET, KERNEL_OFFSET, build, symbols, boot_image, kernel_output

USER_START = 0x400000
BOOTINFO = 0x601000
IPC = 0x600000
EXTRA = 0x602000
STACK_START, STACK_END = 0x5fc000, 0x600000


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


def check_layout(gdb, syms, elf, level, dtb):
    mapping = pages(gdb)
    user = {va: p for va, p in mapping.items() if p[1] & (1 << 6)}
    data = elf.read_bytes()
    phoff = struct.unpack_from('<Q', data, 32)[0]
    phnum = struct.unpack_from('<H', data, 56)[0]
    expected = {BOOTINFO: 4, IPC: 6, **{va: 6 for va in range(STACK_START, STACK_END, 4096)}}
    extra_size = len(dtb) + 16
    extra_pages = (extra_size + 4095) // 4096 * 4096
    expected.update({va: 4 for va in range(EXTRA, EXTRA + extra_pages, 4096)})
    for i in range(phnum):
        typ, flags, _, va, _, _, size, _ = struct.unpack_from('<IIQQQQQQ', data, phoff + i * 56)
        if typ == 1:
            for address in range(va, (va + size + 4095) // 4096 * 4096, 4096):
                expected[address] = flags
    assert user.keys() == expected.keys(), (user.keys() - expected.keys(), expected.keys() - user.keys())
    assert len({pa for pa, _ in user.values()}) == len(user), 'aliased user frames'
    for va, flags in expected.items():
        pa, attr = user[va]
        if va in (BOOTINFO, IPC) or EXTRA <= va < EXTRA + extra_pages:
            assert syms['__frames_start'] - KERNEL_OFFSET <= pa < syms['__frames_end'] - KERNEL_OFFSET
        else:
            assert gdb.command('Qqemu.PhyMemMode:1') == 'OK'
            offset = gdb.word(syms['LOADER_BOOT_INFO'] - KERNEL_OFFSET + 16)
            assert gdb.command('Qqemu.PhyMemMode:0') == 'OK'
            assert pa == va + offset, 'loader image was copied instead of adopted'
        assert attr & (1 << 53), 'user pages must be PXN'
        assert bool(attr & (1 << 7)) == (flags != 6)
        assert bool(attr & (1 << 54)) == (flags != 5)
    for va in (syms['_start'], syms['boot_stack'], 0x09000000):
        assert va not in mapping, 'kernel/MMIO present in user TTBR0'
    assert STACK_START - 4096 not in mapping
    bi = struct.unpack('<12Q', gdb.memory(BOOTINFO, 96))
    assert bi[:6] == (0x525354494e594249, 2, 96, 4096, int(level != 'off'), IPC)
    assert bi[6] == USER_START and bi[8:] == (STACK_START, STACK_END, EXTRA, extra_size)
    assert gdb.memory(BOOTINFO + 96, 4096 - 96) == bytes(4096 - 96)
    assert gdb.memory(EXTRA, 16) == struct.pack('<QQ', 6, extra_size)
    assert gdb.memory(EXTRA + 16, len(dtb)) == dtb, 'DTB changed during BootInfo transfer'
    assert gdb.memory(EXTRA + extra_size, extra_pages - extra_size) == bytes(extra_pages - extra_size)


def run(qemu, kernel, user, level, scenario):
    ks, us = symbols(kernel), symbols(user)
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
                assert gdb.reg('sp') == 0 and gdb.reg('x0') == BOOTINFO
                assert all(gdb.reg(f'x{i}') == 0 for i in range(1, 31)), 'stale initial GPR'
                assert gdb.command('Qqemu.PhyMemMode:1') == 'OK'
                assert gdb.word(ks['SCHEDULER'] - KERNEL_OFFSET) == 1
                assert gdb.command('Qqemu.PhyMemMode:0') == 'OK'
                # The user runtime, rather than the kernel, installs its ELF-owned stack.
                assert gdb.command('s').startswith(('T05', 'S05'))
                assert gdb.command('s').startswith(('T05', 'S05'))
                assert gdb.reg('sp') == STACK_END
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
                    assert user_word(us['RESULT']) == 0
                    if scenario == 'ignored-dtb':
                        write(gdb, EXTRA + 16, bytes(4))
                    hello = user.parents[2] / 'hello.elf'
                    hello_data = hello.read_bytes()
                    entry = struct.unpack_from('<Q', hello_data, 24)[0]
                    gdb.run_to(entry)
                    assert gdb.reg('cpsr') & 15 == 0
                    assert gdb.reg('sp') == 0x8000000 and gdb.reg('x0') == 0
                    child_pages = pages(gdb)
                    assert BOOTINFO not in child_pages and USER_START not in child_pages
                    assert 0x7ffb000 not in child_pages, 'missing child stack guard'
                    phoff = struct.unpack_from('<Q', hello_data, 32)[0]
                    phnum = struct.unpack_from('<H', hello_data, 56)[0]
                    expected = set(range(0x7ffc000, 0x8000000, 4096))
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
                    assert gdb.memory(0x7ffc000, 16384) == bytes(16384)
                    for _ in range(10):
                        gdb.run_to(ks['root_idle'])
                        if gdb.word(ks['SCHEDULER']) == 2:
                            break
                        assert gdb.word(ks['SCHEDULER']) == 5, 'unexpected root stop'
                        assert gdb.command('s').startswith(('T05', 'S05'))
                    assert gdb.word(ks['SCHEDULER']) == 2
                    assert user_word(us['RESULT']) == 1
                    assert user_word(us['BOOTINFO_ADDRESS']) == BOOTINFO
                    assert user_word(IPC) == 0
                elif scenario in ('invalid-bootinfo', 'invalid-extra', 'invalid-hello', 'invalid-hello-entry'):
                    # Corrupt the version through the debugger, before the
                    # runtime constructs its safe BootInfo view.
                    if scenario == 'invalid-bootinfo':
                        write(gdb, BOOTINFO + 8, struct.pack('<Q', 999))
                    elif scenario == 'invalid-extra':
                        write(gdb, EXTRA + 8, struct.pack('<Q', 0))
                    elif scenario == 'invalid-hello':
                        write(gdb, us['__hello_start'], bytes(4))
                    else:
                        write(gdb, us['__hello_start'] + 24, struct.pack('<Q', 0))
                    gdb.run_to(ks['root_idle'])
                    assert gdb.word(ks['SCHEDULER']) == 2
                    assert user_word(us['RESULT']) == 0
                    assert user_word(us['BOOTINFO_ADDRESS']) == (BOOTINFO if scenario.startswith('invalid-hello') else 0)
                    assert proc.poll() is None
                elif scenario == 'svc-registers':
                    # Patch a scratch instruction at the user entry through the
                    # debugger; guest writes to this RX page are tested below.
                    write(gdb, us['_start'], struct.pack('<I', 0xd4000001)) # svc #0
                    for number, argument, result in [(0, 17, 0), (999, 0, 1), (1, 256, 2)]:
                        values = {f'x{i}': 0x12340000 + i for i in range(31)}
                        values['x0'], values['x8'] = argument, number
                        for reg, value in values.items():
                            gdb.write_reg(reg, value)
                        gdb.write_reg('pc', us['_start'])
                        gdb.run_to(us['_start'] + 4)
                        assert gdb.reg('cpsr') & 15 == 0
                        assert gdb.reg('sp') == STACK_END
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
                    target, instruction, ec, permission = {
                        'kernel-read': (ks['_start'], 0xf9400001, 0x24, True),
                        'uart-read': (0x09000000, 0xf9400001, 0x24, False),
                        'text-write': (us['_start'], 0xf9000001, 0x24, True),
                        'bootinfo-write': (BOOTINFO, 0xf9000001, 0x24, True),
                        'dtb-write': (EXTRA + 16, 0xf9000001, 0x24, True),
                        'guard-read': (STACK_START - 4096, 0xf9400001, 0x24, False),
                        'null-read': (0, 0xf9400001, 0x24, False),
                        'stack-execute': (STACK_START, 0xd61f0000, 0x20, True),
                    }[scenario]
                    write(gdb, us['_start'], struct.pack('<I', instruction))
                    gdb.write_reg('x0', target)
                    gdb.run_to(ks['root_idle'])
                    assert gdb.word(ks['SCHEDULER']) == 3, 'faulting root task not stopped'
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
