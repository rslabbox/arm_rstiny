#!/usr/bin/env python3
"""Exercise task/memory syscalls on real EL0 CPUs, including timer-only workers."""
import argparse
from pathlib import Path
import struct
import subprocess
import tempfile
from check_kernel import Gdb, build, boot_image, mappings
from check_fatboot import write

from elf_image import parse_elf, root_layout
CODE, DATA, STACK = 0x1000000, 0x1100000, 0x1200000
PAGE = 4096


def mov(register, value):
    instructions = [0xd2800000 | ((value & 65535) << 5) | register]
    for shift in range(1, 4):
        part = (value >> (16 * shift)) & 65535
        if part:
            instructions.append(0xf2800000 | (shift << 21) | (part << 5) | register)
    return instructions


def run(qemu, kernel):
    directory = boot_image(kernel).parent
    root_image = parse_elf((directory / 'rootserver').read_bytes())
    layout = root_layout(root_image, (directory / 'kernel.dtb').stat().st_size)
    entry, buffer = root_image['entry'], layout['ipc']
    with tempfile.TemporaryDirectory(prefix='rstiny-tasks-') as temp:
        temp = Path(temp)
        serial = temp / 'serial'
        with (temp / 'errors').open('wb') as errors:
            proc = subprocess.Popen([
                qemu, '-machine', "virt,gic-version=3,virtualization=off",
                '-cpu', 'cortex-a72', '-smp', '1', '-m', '128M', '-display', 'none',
                '-monitor', 'none', '-serial', f'file:{serial}', '-nic', 'none',
                '-kernel', str(boot_image(kernel)), '-S', '-gdb', f'unix:{temp / "gdb"},server=on,wait=off',
            ], stderr=errors, stdout=subprocess.DEVNULL)
            gdb = None
            try:
                gdb = Gdb(temp / 'gdb', proc)
                gdb.run_to(entry)
                assert gdb.reg('cpsr') & 0x8f == 0, 'EL0 IRQs must be enabled'
                # Verify the actual GICv3 hardware state before exercising scheduling.
                assert gdb.command('Qqemu.PhyMemMode:1') == 'OK'
                def mmio32(address):
                    return int.from_bytes(gdb.memory(address, 4), 'little')
                assert (mmio32(0x0800ffe8) >> 4) & 15 == 3, 'expected GICv3'
                assert mmio32(0x08000000) & 0x12 == 0x12, 'Group 1/affinity routing disabled'
                assert mmio32(0x080a0014) & 6 == 0, 'redistributor asleep'
                assert mmio32(0x080b0080) & (1 << 30), 'timer must use Group 1'
                assert mmio32(0x080b0100) == 1 << 30, 'only the timer PPI should be enabled'
                assert mmio32(0x080b0c04) & (3 << 28) == 0, 'timer must be level-triggered'
                assert gdb.command('Qqemu.PhyMemMode:0') == 'OK'
                write(gdb, entry, struct.pack('<I', 0xd4000001))

                def call(number, *args, status=0):
                    for i in range(5):
                        gdb.write_reg(f'x{i}', args[i] if i < len(args) else 0)
                    gdb.write_reg('x8', number)
                    gdb.write_reg('pc', entry)
                    gdb.run_to(entry + 4)
                    actual = gdb.reg('x0')
                    assert actual == status, (number, args, actual, status)
                    assert gdb.reg('cpsr') & 15 == 0
                    return gdb.reg('x1')

                root = call(3)
                # Extra BootInfo remains immutable for the runtime's DTB slice.
                call(13, root, layout['extra'], PAGE, status=6)
                call(14, root, layout['extra'], PAGE, 3, status=6)
                baseline = call(17)
                child = call(4)
                assert call(17) == baseline - 3
                call(12, root + 32, DATA, PAGE, 3, status=7) # stale/forged handle
                for address, length, rights in [(0, PAGE, 3), (DATA + 1, PAGE, 3),
                                                (DATA, 0, 3), (DATA, PAGE, 7),
                                                (0x8000000, PAGE, 3), (2**64-4096, PAGE, 3)]:
                    call(12, child, address, length, rights, status=2)
                call(12, child, DATA, 2 * PAGE, 3)
                after_map = call(17)
                call(12, child, DATA, PAGE, 3, status=5)
                assert call(17) == after_map
                payload = bytes(range(32))
                write(gdb, buffer, payload)
                call(15, child, DATA + PAGE - 16, buffer, len(payload))
                call(16, child, DATA + PAGE - 16, buffer + 128, len(payload))
                assert gdb.memory(buffer + 128, len(payload)) == payload
                call(14, child, DATA + PAGE, PAGE, 1)
                write(gdb, buffer, bytes([255]) * 32)
                call(15, child, DATA + PAGE - 16, buffer, 32, status=6)
                call(16, child, DATA + PAGE - 16, buffer + 128, 32)
                assert gdb.memory(buffer + 128, 32) == payload, 'partial failed write'
                call(15, child, DATA, 0x40000000, 8, status=2) # invalid source pointer
                call(16, child, DATA, 0x40000000, 8, status=2) # invalid destination
                call(15, child, DATA, buffer, 4097, status=2)
                # Wrapping an address must not bypass range or permission checks.
                for invalid in (0, 2**64 - 8):
                    call(15, child, DATA, invalid, 32, status=2)
                    call(16, child, DATA, invalid, 32, status=2)
                    call(15, child, invalid, buffer, 32, status=2)
                    call(16, child, invalid, buffer, 32, status=2)
                call(16, child, DATA, entry, 8, status=6) # caller RX destination
                for number in (15, 16):
                    write(gdb, buffer, payload + bytes(16))
                    if number == 15:
                        call(number, root, buffer + 8, buffer, len(payload))
                    else:
                        call(number, root, buffer, buffer + 8, len(payload))
                    assert gdb.memory(buffer + 8, len(payload)) == payload, 'overlapping self-copy'
                call(13, child, DATA, 3 * PAGE, status=4)
                call(13, root, layout['boot_info'], PAGE, status=6) # lifetime-pinned BootInfo
                call(14, root, layout['boot_info'], PAGE, 3, status=6)
                call(13, child, DATA, 2 * PAGE)
                call(12, child, DATA, PAGE, 3)
                call(16, child, DATA, buffer, 64)
                assert gdb.memory(buffer, 64) == bytes(64), 'recycled frame leaked data'
                call(8, child)
                assert call(17) == baseline, 'destroy leaked page tables or frames'
                call(9, child, status=7)

                # Allocation failure must release all staging frames and tables.
                large = call(4)
                other = call(4)
                filler = call(4)
                call(12, large, CODE, 1024 * PAGE, 3)
                call(12, filler, CODE, 512 * PAGE, 3)
                before_failure = call(17)
                call(12, other, CODE, 1024 * PAGE, 3, status=3)
                assert call(17) == before_failure, 'failed mapping leaked frames'
                call(16, other, CODE, buffer, 8, status=4)
                call(12, other, CODE, 1025 * PAGE, 3, status=3)
                call(8, large)
                call(8, other)
                call(8, filler)
                assert call(17) == baseline

                handles = [call(4) for _ in range(31)]
                before_failure = call(17)
                call(4, status=3)
                assert call(17) == before_failure, 'task limit leaked root tables'
                for handle in handles:
                    call(8, handle)
                assert call(17) == baseline

                def task(instructions):
                    handle = call(4)
                    call(12, handle, CODE, PAGE, 3)
                    call(12, handle, DATA, PAGE, 3)
                    call(12, handle, STACK, PAGE, 3)
                    code = struct.pack('<' + 'I' * len(instructions), *instructions)
                    write(gdb, buffer, code)
                    call(15, handle, CODE, buffer, len(code))
                    call(14, handle, CODE, PAGE, 5)
                    return handle

                # No SVC/yield in either worker: only timer IRQs can regain root.
                spin = mov(9, DATA) + [0xf940012a, 0x9100054a, 0xf900012a, 0x17fffffd]
                a, b = task(spin), task(spin)
                call(5, a, DATA, STACK + PAGE, 0, status=6) # NX entry rejected
                call(5, a, CODE, STACK + PAGE - 1, 0, status=2)
                call(7, a, status=8)
                call(5, a, CODE, STACK + PAGE, 0)
                call(5, b, CODE, STACK + PAGE, 0)
                call(12, a, DATA + PAGE, PAGE, 3, status=9) # cannot edit runnable child
                call(11, 30)
                call(6, a)
                call(6, b)
                call(16, a, DATA, buffer, 8)
                count_a = gdb.word(buffer)
                call(16, b, DATA, buffer, 8)
                count_b = gdb.word(buffer)
                assert count_a > 0 and count_b > 0, 'timer preemption did not run both tasks'
                write(gdb, buffer, struct.pack('<Q', 0xfeed))
                call(15, a, DATA, buffer, 8)
                call(16, b, DATA, buffer, 8)
                assert gdb.word(buffer) == count_b, 'address spaces alias'
                before = call(18)
                call(11, 25) # all other tasks suspended: timer must wake an idle CPU
                assert call(18) - before >= 25
                call(7, a)
                call(11, 20)
                call(6, a)
                call(16, a, DATA, buffer, 8)
                assert gdb.word(buffer) not in (0, 0xfeed), 'resumed context did not continue'
                call(8, a)
                call(7, b)
                call(8, b) # remove a ready task from the queue before freeing its space
                assert call(17) == baseline

                # Blocking wait, exit/reap, parent ownership and user fault isolation.
                sleeper = task(mov(0, 25) + mov(8, 11) + [0xd4000001] +
                               mov(0, 42) + mov(8, 10) + [0xd4000001, 0x14000000])
                call(5, sleeper, CODE, STACK + PAGE, 0)
                assert call(19, sleeper) == 42
                assert call(9, sleeper) == 6
                assert call(17) == baseline, 'exit must release address space before reap'
                call(8, sleeper)
                # A paused sleeper must retain its deadline across resume.
                sleeper = task(mov(0, 100) + mov(8, 11) + [0xd4000001] +
                               mov(0, 43) + mov(8, 10) + [0xd4000001])
                before = call(18)
                call(5, sleeper, CODE, STACK + PAGE, 0)
                call(6, sleeper)
                assert call(9, sleeper) == 2
                call(7, sleeper)
                assert call(19, sleeper) == 43
                assert call(18) - before >= 100
                call(8, sleeper)

                # Complete a wait while its caller is suspended. The scheduler
                # retains a value, and dispatch encodes it only on resumption.
                grandchild_code = mov(0, 200) + mov(8, 11) + [0xd4000001]
                grandchild_code += mov(0, 42) + mov(8, 10) + [0xd4000001]
                parent_code = mov(8, 4) + [0xd4000001, 0xaa0103f3] # x19 = child
                parent_code += mov(9, DATA) + [0xf9000133] # save handle
                def child_call(number, *arguments):
                    code = [0xaa1303e0] # x0 = x19
                    for register, argument in enumerate(arguments, 1):
                        code += mov(register, argument)
                    return code + mov(8, number) + [0xd4000001]
                parent_code += child_call(12, CODE, PAGE, 3)
                parent_code += child_call(15, CODE, CODE + 1024, len(grandchild_code) * 4)
                parent_code += child_call(14, CODE, PAGE, 5)
                parent_code += child_call(12, STACK, PAGE, 3)
                parent_code += child_call(5, CODE, STACK + PAGE, 0)
                parent_code += child_call(19)
                parent_code += [0xaa0103e0] + mov(8, 10) + [0xd4000001]
                assert len(parent_code) < 256
                parent_code += [0] * (256 - len(parent_code)) + grandchild_code
                waiter = task(parent_code)
                call(5, waiter, CODE, STACK + PAGE, 0)
                for _ in range(30):
                    if call(9, waiter) == 7:
                        break
                    call(11, 1)
                assert call(9, waiter) == 7
                call(6, waiter)
                call(16, waiter, DATA, buffer, 8)
                grandchild = gdb.word(buffer)
                call(11, 250)
                assert call(9, waiter) == 2, 'completion resumed a suspended waiter'
                call(7, waiter)
                assert call(19, waiter) == 42, 'deferred wait result was lost'
                assert call(9, grandchild) == 6 # adopted after parent exit
                call(8, grandchild)
                call(8, waiter)
                assert call(17) == baseline

                # A suspended waiter must resume waiting rather than return a
                # fabricated result. Its unstarted child is adopted on destroy.
                waiter_code = mov(8, 4) + [0xd4000001] + mov(9, DATA) + [0xf9000121, 0xaa0103e0]
                waiter_code += mov(8, 19) + [0xd4000001, 0xaa0103e0] + mov(8, 10) + [0xd4000001]
                waiter = task(waiter_code)
                call(5, waiter, CODE, STACK + PAGE, 0)
                for _ in range(10):
                    if call(9, waiter) == 7:
                        break
                    call(11, 10)
                assert call(9, waiter) == 7
                call(6, waiter)
                call(16, waiter, DATA, buffer, 8)
                adopted = gdb.word(buffer)
                call(9, adopted, status=6)
                call(7, waiter)
                assert call(9, waiter) == 7
                call(8, waiter)
                assert call(9, adopted) == 0
                call(8, adopted)
                unauthorized = task(mov(0, root) + mov(8, 9) + [0xd4000001] + mov(8, 10) + [0xd4000001])
                call(5, unauthorized, CODE, STACK + PAGE, 0)
                assert call(19, unauthorized) == 6, 'child gained parent authority'
                call(8, unauthorized)
                fault = task(mov(9, 0) + [0xf9400120])
                call(5, fault, CODE, STACK + PAGE, 0)
                assert call(19, fault) >> 26 == 0x24
                assert call(9, fault) == 3
                assert call(17) == baseline, 'faulted task retained user memory'
                call(8, fault)
                for instruction, ec in [(0xd51be220, 0x18), (0x9e670000, 0x07)]:
                    # MSR CNTP_CTL_EL0 and FMOV d0,x0 must trap, not defeat
                    # preemption or use an unsaved FP register bank.
                    denied = task([instruction, 0x14000000])
                    call(5, denied, CODE, STACK + PAGE, 0)
                    assert call(19, denied) >> 26 == ec
                    assert call(9, denied) == 3
                    call(8, denied)
                # Every started task owns a persistent, guarded kernel stack.
                # Park all available children, then reclaim their stack aliases.
                before_stacks = mappings(gdb)
                sleepers = [task(mov(0, 60000) + mov(8, 11) + [0xd4000001, 0x14000000])
                            for _ in range(31)]
                for sleeper in sleepers:
                    call(5, sleeper, CODE, STACK + PAGE, 0)
                    # An IRQ can preempt the first entry before its Sleep SVC.
                    for _ in range(20):
                        state = call(9, sleeper)
                        if state != 4:
                            break
                        call(0)
                    assert state == 5, f'sleeper state {state}'
                assert len(before_stacks.keys() - mappings(gdb).keys()) == 2 * 31
                for sleeper in sleepers:
                    call(8, sleeper)
                assert mappings(gdb) == before_stacks, 'destroy failed to restore kernel stack aliases'
                assert call(17) == baseline

                # More than the 16 MiB heap could retain if 64 KiB stacks leaked.
                for _ in range(260):
                    exited = task(mov(0, 42) + mov(8, 10) + [0xd4000001])
                    call(5, exited, CODE, STACK + PAGE, 0)
                    assert call(19, exited) == 42
                    call(8, exited)
                assert mappings(gdb) == before_stacks
                replacement = call(4)
                assert replacement != fault
                call(9, fault, status=7)
                call(8, replacement)
                assert call(17) == baseline
                assert proc.poll() is None
            except Exception:
                print(serial.read_text(errors='replace') if serial.exists() else '', flush=True)
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
            print(f'CHECK memory/tasks {mode} LOG={level}', flush=True)
            run(args.qemu, kernel)
    print('PASS: memory transactions/recycling; task ownership/lifecycle; timer preemption; idle wakeup; wait/fault isolation.', flush=True)


if __name__ == '__main__':
    main()
