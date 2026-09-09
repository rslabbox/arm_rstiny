#!/usr/bin/env python3
"""Verify returning EL0 execution preserves the kernel continuation and stack."""
import argparse
from pathlib import Path
import struct
import subprocess
import tempfile
from check_kernel import Gdb, build, boot_image, symbols, translate, KERNEL_OFFSET
from check_fatboot import write
from elf_image import parse_elf


# Independent wire fixture for the C-layout kernel continuation record.
CONTEXT_FIELDS = tuple(f'x{i}' for i in range(19, 29)) + ('fp', 'lr', 'sp')
CONTEXT_LAYOUT = struct.Struct('<13Q')


def kernel_context(gdb, address):
    assert address % 8 == 0
    return dict(zip(CONTEXT_FIELDS, CONTEXT_LAYOUT.unpack(gdb.memory(address, CONTEXT_LAYOUT.size))))


def run(qemu, kernel):
    syms = symbols(kernel)
    entry = parse_elf((boot_image(kernel).parent / 'userboot').read_bytes())['entry']
    with tempfile.TemporaryDirectory(prefix='rstiny-context-') as directory:
        directory = Path(directory)
        proc = subprocess.Popen([
            qemu, '-machine', 'virt,gic-version=3,virtualization=off', '-cpu', 'cortex-a72',
            '-smp', '1', '-m', '128M', '-display', 'none', '-monitor', 'none',
            '-serial', f'file:{directory / "serial"}', '-nic', 'none', '-kernel', str(boot_image(kernel)),
            '-S', '-gdb', f'unix:{directory / "gdb"},server=on,wait=off',
        ], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        gdb = None
        try:
            gdb = Gdb(directory / 'gdb', proc)
            gdb.run_to(syms['switch_kernel_context'])
            if 'IRQ_SELF_TEST_PASSED' in syms:
                assert gdb.word(syms['IRQ_SELF_TEST_PASSED']) == 1, 'GIC/timer lifecycle self-tests did not complete'
            scheduler_sp = gdb.reg('sp')
            assert syms['boot_stack'] <= scheduler_sp < syms['boot_stack_top']
            initial = kernel_context(gdb, gdb.reg('x1'))
            stack_top = initial['sp']
            assert initial['x19'] != 0, 'missing trampoline argument'
            assert syms['skernel'] <= initial['lr'] < syms['etext'], 'invalid trampoline'
            assert all(initial[field] == 0 for field in CONTEXT_FIELDS if field not in ('x19', 'lr', 'sp'))
            stack_bottom = stack_top - 64 * 1024
            assert syms['__heap_start'] <= stack_bottom < stack_top <= syms['__heap_end']
            guard = stack_bottom - 4096
            physical_guard = translate(gdb, stack_bottom) - 4096
            for address in (guard, KERNEL_OFFSET + physical_guard):
                try:
                    translate(gdb, address)
                except AssertionError as error:
                    assert 'unmapped kernel VA' in str(error)
                else:
                    raise AssertionError('task stack guard still mapped')
            gdb.run_to(syms['run_user'])
            # The user mapping is already active, but no EL0 instruction has run.
            # Inject yield in a loop, then separately a timer-only spin loop.
            write(gdb, entry, struct.pack('<II', 0xd4000001, 0x17ffffff))
            frame = gdb.reg('x0')
            write(gdb, frame + 7 * 8, struct.pack('<Q', (1 << 64)-7))  # saved user x7 = seL4 Yield
            baseline = gdb.reg('sp')
            assert baseline % 16 == 0
            assert stack_bottom <= baseline < stack_top
            # Preserve a suspended Rust caller through task -> scheduler -> task.
            gdb.run_to(syms['switch_kernel_context'])
            task_sp, task_return = gdb.reg('sp'), gdb.reg('x30')
            assert stack_bottom <= task_sp < stack_top
            assert kernel_context(gdb, gdb.reg('x1'))['sp'] == scheduler_sp
            outgoing_context = gdb.reg('x0')
            saved = {f'x{i}': gdb.reg(f'x{i}') for i in range(19, 30)}
            for i, register in enumerate(saved):
                gdb.write_reg(register, 0xdef00000 + i)
            gdb.run_to(task_return)
            record = kernel_context(gdb, outgoing_context)
            assert record['sp'] == task_sp and record['lr'] == task_return
            for i, register in enumerate(saved):
                assert record['fp' if register == 'x29' else register] == 0xdef00000 + i
            assert gdb.reg('sp') == task_sp and gdb.reg('cpsr') & 0x8f == 0x85
            for i, (register, value) in enumerate(saved.items()):
                assert gdb.reg(register) == 0xdef00000 + i, register
                gdb.write_reg(register, value)
            gdb.run_to(syms['run_user'])
            seen = set()
            for iteration in range(2048):
                if iteration:
                    gdb.run_to(syms['run_user'])
                assert gdb.reg('sp') == baseline, 'kernel call stack grew across traps'
                assert gdb.reg('cpsr') & 0x8f == 0x85, 'entry must be EL1h, IRQ masked'
                return_pc = gdb.reg('x30')
                result = gdb.reg('x1')
                frame = gdb.reg('x0')
                # A timer-only worker proves run returns without a user SVC.
                if iteration == 2040:
                    write(gdb, entry, struct.pack('<I', 0x14000000))
                    write(gdb, gdb.reg('x0') + 256, struct.pack('<Q', entry))
                saved = {}
                if iteration % 128 == 0 or iteration >= 2040:
                    saved = {f'x{i}': gdb.reg(f'x{i}') for i in range(19, 30)}
                    for i, register in enumerate(saved):
                        gdb.write_reg(register, 0xabc00000 + i)
                gdb.run_to(return_pc)
                assert gdb.reg('sp') == baseline, 'run did not restore its caller SP'
                assert gdb.reg('cpsr') & 0x8f == 0x85
                kind = gdb.word(result)
                seen.add(kind)
                assert kind in (0, 1)
                if iteration >= 2040:
                    assert kind == 1, 'timer-only EL0 did not return through IRQ'
                for i, (register, value) in enumerate(saved.items()):
                    assert gdb.reg(register) == 0xabc00000 + i, register
                    # Restore the real Rust caller's variables before it executes.
                    gdb.write_reg(register, value)
            assert seen == {0, 1}
            # Stop root, then force a timer pending *at* masked WFI. This
            # catches the lost-wakeup window that ordinary sleep tests miss.
            write(gdb, entry, struct.pack('<I', 0xd4000001))
            write(gdb, frame + 7 * 8, struct.pack('<Q', (1 << 64)-1))
            write(gdb, frame, struct.pack('<QQ', 1, 11 << 12)) # Call self TCB.Suspend
            write(gdb, frame + 256, struct.pack('<Q', entry))
            gdb.run_to(syms['root_idle'])
            assert gdb.reg('x0') == 2
            idle_sp = gdb.reg('sp')
            for _ in range(2):
                gdb.write_reg('cntp_ctl_el0', 0)
                for _ in range(256):
                    if gdb.memory(gdb.reg('pc'), 4) == struct.pack('<I', 0xd503207f):
                        break
                    assert gdb.command('s').startswith(('T05', 'S05'))
                else:
                    raise AssertionError('idle did not reach WFI')
                assert gdb.reg('cpsr') & 0x80
                gdb.write_reg('cntp_cval_el0', 0)
                gdb.write_reg('cntp_ctl_el0', 1)
                gdb.run_to(syms['root_idle'])
                assert gdb.reg('sp') == idle_sp and gdb.reg('x0') == 2

        except Exception:
            print((directory / 'serial').read_text(errors='replace'), flush=True)
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
            kernel = build(mode, level, True)
            print(f'CHECK returning context {mode} LOG={level}', flush=True)
            run(args.qemu, kernel)
    print('PASS: GIC claim/complete, private/shared masking, unknown IRQ routing, one-shot timer; 2048 returns per configuration; private guarded task stack, kernel continuation switch, callee-saved registers, SP and IRQ state; SVC and timer-only EL0; pending-before-WFI idle wakeup.', flush=True)


if __name__ == '__main__':
    main()
