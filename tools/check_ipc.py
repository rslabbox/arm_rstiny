#!/usr/bin/env python3
"""Exercise endpoint IPC, badges, notifications, cap transfer and fault
endpoints on real EL0 tasks."""
import argparse
import struct
import subprocess
import tempfile
from pathlib import Path

from check_kernel import Gdb, build, boot_image
from check_fatboot import write
from elf_image import parse_elf, root_layout
from abi_client import Client, mov, invoke_code, WORD

CODE, DATA, STACK = 0x1000000, 0x1100000, 0x1200000
FAULT_VA = 0x2000000
PAGE = 4096
IPC_VA = 0x8000000 - PAGE  # managed children's IPC buffer (USER_ADDRESS_LIMIT - PAGE)

CALL, REPLYRECV, SEND, NBSND, RECV, REPLY, NBRECV = (WORD - k for k in (0, 1, 2, 3, 4, 5, 7))
TASK_SUSPENDED, TASK_BLOCKED_RECV, TASK_BLOCKED_FAULT = 2, 9, 11
FAULT_VM, FAULT_UNKNOWN_SYSCALL = 4, 2
SVC = 0xd4000001


def mov_reg(rd, rn):
    return [0xAA000000 | (rn << 5) | rd]


def sysipc(cap, tag, syscall, x2=0):
    return mov(0, cap) + mov(1, tag) + mov(2, x2) + mov(7, syscall) + [SVC]


class IpcClient(Client):
    """Adds raw (possibly blocking-free) syscall execution to the root task."""

    def sysc(self, cap, syscall, tag=0, x2=0, caps=()):
        g = self.gdb
        words = list(caps) + [0] * (3 - len(caps))
        for index, value in enumerate(words[:3]):
            write(g, self.ipc + 976 + index * 8, struct.pack('<Q', value))
        g.write_reg('x0', cap)
        g.write_reg('x1', tag)
        g.write_reg('x2', x2)
        g.write_reg('x7', syscall)
        g.write_reg('pc', self.entry)
        g.run_to(self.entry + 4)
        return [g.reg(f'x{index}') for index in range(6)]

    def nbrecv(self, cap):
        badge, tag = self.sysc(cap, NBRECV)[:2]
        return badge, tag >> 12, tag & 0x7F


def run(qemu, kernel):
    directory = boot_image(kernel).parent
    image = parse_elf((directory / 'userboot').read_bytes())
    ipc = root_layout(image, (directory / 'kernel.dtb').stat().st_size)['ipc']
    with tempfile.TemporaryDirectory(prefix='rstiny-ipc-') as temporary:
        tmp = Path(temporary)
        serial = tmp / 'serial'
        proc = subprocess.Popen([
            qemu, '-machine', 'virt,gic-version=3,virtualization=off', '-cpu', 'cortex-a72',
            '-smp', '1', '-m', '128M', '-display', 'none', '-monitor', 'none', '-nic', 'none',
            '-serial', f'file:{serial}', '-kernel', str(boot_image(kernel)),
            '-S', '-gdb', f'unix:{tmp / "gdb"},server=on,wait=off',
        ], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        g = None
        try:
            g = Gdb(tmp / 'gdb', proc)
            g.run_to(image['entry'])
            c = IpcClient(g, image['entry'], ipc)
            baseline = c.runtime('available')
            children = []

            def retype(kind, slot, bits=0, count=1, alloc=32, status=0):
                return c.call(alloc, 1, [kind, bits, 0, 0, slot, count], [2], status)

            def mint(dst_cnode, dst, src, rights=15, badge=0, status=0):
                return c.call(dst_cnode, 21, [dst, 64, src, 64, rights, badge], [2], status)

            def delete(slot, status=0):
                c.call(2, 18, [slot, 64], status=status)

            def revoke(slot, status=0):
                c.call(2, 17, [slot, 64], status=status)

            def child(words):
                """A managed task with a code page; cap 140 is reserved for a
                badged endpoint copy minted after creation."""
                handle = c.runtime('create')
                for address in (CODE, DATA, STACK):
                    c.runtime('map', handle, address, PAGE, 3)
                code = struct.pack('<' + 'I' * len(words), *words)
                write(g, ipc, code)
                c.runtime('write', handle, CODE, ipc, 4 * len(words))
                c.runtime('protect', handle, CODE, PAGE, 5)
                children.append(handle)
                return handle

            def start(handle):
                c.runtime('start', handle, CODE, STACK + PAGE, 0)

            def wait_state(handle, state, code):
                for _ in range(40):
                    if c.runtime('status', handle) == state:
                        return
                    c.runtime('sleep', 1)
                raise AssertionError(('task did not reach state', code, state))

            def receive_badge(code, sender=None):
                seen = None
                for _ in range(2000):
                    seen = c.nbrecv(120)
                    if seen[0] or seen[1]:
                        return seen
                    c.runtime('sleep', 1)
                state = sender is not None and c.runtime('status', sender)
                raise AssertionError(('no delivery', code, seen, state))

            # Objects and badges. Root's endpoint/notification caps are
            # unbadged at slots 120/121; children receive minted copies.
            retype(2, 120)
            retype(3, 121)
            mint(2, 130, 120, badge=7)
            mint(2, 131, 121, badge=11)
            # seL4 updateCapData: a badge only mints onto an unbadged source.
            mint(2, 132, 130, badge=9, status=3)
            assert c.nbrecv(120) == (0, 0, 0), 'empty endpoint must not deliver'

            # A queued Call delivers badge and message; the reply resumes the
            # caller with the reply message registers.
            sender = child(sysipc(140, (0x200 << 12) | 1, CALL, 41) +
                           invoke_code('exit', [('reg', 2)]))
            node = c.runtime('cspace', sender)
            mint(node, 140, 130)
            start(sender)
            got = receive_badge('queued call', sender)
            assert got == (7, 0x200, 1), ('queued call mismatch', got)
            c.sysc(0, REPLY, (0x300 << 12) | 1, 42)
            assert c.runtime('wait', sender) == 42, 'reply message'

            # NBSend without a waiter is a successful no-op.
            c.sysc(120, NBSND, 0, 5)
            assert c.nbrecv(120) == (0, 0, 0)

            # A signal with no waiter merges into the notification bits; a
            # later waiter collects them. A signal to a blocked waiter wakes
            # it directly.
            merged = child(sysipc(140, 0, RECV) + mov_reg(2, 0) +
                           invoke_code('exit', [('reg', 2)]))
            node = c.runtime('cspace', merged)
            mint(node, 140, 131)
            c.sysc(131, SEND)
            start(merged)
            assert c.runtime('wait', merged) == 11, 'merged notification badge'

            direct = child(sysipc(140, 0, RECV) + mov_reg(2, 0) +
                           invoke_code('exit', [('reg', 2)]))
            node = c.runtime('cspace', direct)
            mint(node, 140, 131)
            start(direct)
            wait_state(direct, TASK_BLOCKED_RECV, 'blocked notification waiter')
            c.sysc(131, SEND)
            assert c.runtime('wait', direct) == 11, 'direct notification wakeup'

            # A page fault reaches the task's fault endpoint as a VMFault
            # message; the supervisor repairs and replies to restart it.
            faulty = child(mov(8, FAULT_VA) + [0xF9400109] +  # ldr x9, [x8]
                           mov(2, 77) + invoke_code('exit', [('reg', 2)]))
            node = c.runtime('cspace', faulty)
            space = c.runtime('vspace', faulty)
            c.call(6, 48, [], [space])  # ASIDPool_Assign for TCB_Configure
            c.copy(2, 142, 10, source_cnode=node)  # the child's IPC frame cap
            mint(node, 141, 120, badge=21)
            c.call(faulty, 5, [141, 0, 0, IPC_VA], [node, space, 142])
            start(faulty)
            for _ in range(40):
                st = c.runtime('status', faulty)
                if st in (TASK_BLOCKED_FAULT, 3, 2):
                    break
                c.runtime('sleep', 1)
            print('DBG faulty status after start:', st, flush=True)
            assert receive_badge('fault delivery') == (21, FAULT_VM, 4)
            assert g.reg('x3') == FAULT_VA, 'VMFault address'
            assert g.reg('x5') >> 26 == 0x24, 'VMFault FSR'
            assert c.runtime('status', faulty) == TASK_BLOCKED_FAULT
            c.runtime('map', faulty, FAULT_VA, PAGE, 3)
            c.sysc(0, REPLY)
            assert c.runtime('wait', faulty) == 77, 'repaired fault resumed'

            # An unknown syscall faults with its number and restarts past svc.
            stranger = child(mov(7, 0x1234) + [SVC] + mov(2, 5) +
                             invoke_code('exit', [('reg', 2)]))
            node = c.runtime('cspace', stranger)
            space = c.runtime('vspace', stranger)
            c.call(6, 48, [], [space])
            c.copy(2, 143, 10, source_cnode=node)
            mint(node, 141, 120, badge=22)
            c.call(stranger, 5, [141, 0, 0, IPC_VA], [node, space, 143])
            start(stranger)
            assert receive_badge('unknown syscall') == (22, FAULT_UNKNOWN_SYSCALL, 3)
            assert g.reg('x4') == 0x1234, 'unknown syscall number'
            c.sysc(0, REPLY)
            assert c.runtime('wait', stranger) == 5, 'restart past svc'

            # A transfer needs Grant on the source cap and lands through the
            # receiver's receive spec; the received cap is the same frame.
            retype(7, 150)
            physical = c.call(150, 46)
            receiver = child(sysipc(140, 0, RECV) + mov(0, 200) +
                             mov(1, 46 << 12) + mov(7, CALL) + [SVC] +
                             invoke_code('exit', [('reg', 2)]))
            node = c.runtime('cspace', receiver)
            mint(node, 140, 130)
            write(g, ipc + 512, struct.pack('<QQQ', 2, 200, 64))
            c.runtime('write', receiver, IPC_VA + 1000, ipc + 512, 24)
            start(receiver)
            wait_state(receiver, TASK_BLOCKED_RECV, 'transfer receiver')
            c.sysc(120, NBSND, 1 << 7, caps=[150])
            assert c.runtime('wait', receiver) == physical, 'transferred cap identity'

            # Without Grant the whole delivery fails and changes nothing.
            c.copy(2, 151, 150, rights=3)
            denied = child(sysipc(140, 0, RECV) + mov(2, 9) +
                           invoke_code('exit', [('reg', 2)]))
            node = c.runtime('cspace', denied)
            mint(node, 140, 130)
            c.runtime('write', denied, IPC_VA + 1000, ipc + 512, 24)
            start(denied)
            wait_state(denied, TASK_BLOCKED_RECV, 'grant-negative receiver')
            denied_label = c.sysc(120, NBSND, 1 << 7, caps=[151])[1] >> 12
            assert denied_label == 3, ('grant-less transfer must fail', denied_label)
            c.runtime('destroy', denied)
            children.remove(denied)

            # Rights enforcement on endpoints and reply relations.
            mint(2, 152, 120, rights=2)
            assert c.sysc(152, SEND)[1] >> 12 == 3, 'read-only endpoint send'
            assert c.sysc(0, REPLY)[1] >> 12 == 2, 'reply without a caller'

            # Untyped children: split, region revoke, nested teardown, limits.
            retype(0, 160, bits=20)
            retype(2, 161, alloc=160)
            revoke(160)
            c.call(161, 11, status=6)  # the derived endpoint is gone
            retype(2, 162, alloc=160)  # the region resets and is reusable
            retype(0, 163, bits=18, alloc=160)
            retype(2, 164, alloc=163)
            revoke(160)
            c.call(164, 11, status=6)
            retype(2, 162, alloc=160)
            retype(0, 166, bits=25, alloc=160, status=1)  # larger than parent
            retype(0, 166, bits=11, alloc=160, status=1)  # below the minimum
            delete(162)
            delete(160)

            # Deleting the last capability of a populated endpoint suspends the
            # waiters instead of stranding them on a dead object.
            retype(2, 165)
            stranded = child(sysipc(140, 0, RECV) + mov(2, 3) +
                             invoke_code('exit', [('reg', 2)]))
            node = c.runtime('cspace', stranded)
            mint(node, 140, 165)
            start(stranded)
            wait_state(stranded, TASK_BLOCKED_RECV, 'stranded waiter')
            revoke(165)  # removes the child's derived copy
            delete(165)  # last cap: the collected endpoint cancels its waiter
            wait_state(stranded, TASK_SUSPENDED, 'waiter cancelled')
            c.runtime('destroy', stranded)
            children.remove(stranded)

            # Region-granular reclamation, as in check_capabilities.
            for handle in children:
                c.runtime('destroy', handle)
            revoke(32)
            assert c.runtime('available') == baseline, 'IPC test leaked memory'
            assert proc.poll() is None
        except Exception:
            print(serial.read_text(errors='replace'), flush=True)
            raise
        finally:
            if g:
                g.sock.close()
            proc.terminate()
            proc.wait(timeout=5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--qemu', default='qemu-system-aarch64')
    args = parser.parse_args()
    import os
    modes = os.environ.get('IPC_MODES', 'debug,release').split(',')
    levels = os.environ.get('IPC_LEVELS', 'off,info').split(',')
    for mode in modes:
        for level in levels:
            kernel = build(mode, level, False)
            print(f'CHECK ipc {mode} LOG={level}', flush=True)
            run(args.qemu, kernel)
    print('PASS: endpoint call/reply with badges; notification merge and wake; '
          'fault endpoint repair and unknown-syscall restart; cap transfer and '
          'Grant enforcement; Untyped split with nested revoke; waiter cancellation.',
          flush=True)


if __name__ == '__main__':
    main()
