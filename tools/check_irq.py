#!/usr/bin/env python3
"""Exercise IRQ authorization and user-space delivery on real EL0 tasks
(docs/irq.md section 10): platform-table validation, duplicate Get, badge
delivery through a bound Notification, GIC active-state storm protection,
Ack redelivery and Clear quiescing, plus the child-without-IRQControl check.

Interrupt lines are software-pended by writing GICD_ISPENDR through QEMU's
physical memory access, and the line's enable/pending/active latches are read
back the same way, so the whole seL4 GICv3 protocol is observable.
"""
import argparse
import json
import os
import struct
import subprocess
import tempfile
from pathlib import Path

from check_kernel import Gdb, build, boot_image
from check_fatboot import write
from elf_image import parse_elf, root_layout
from abi_client import Client, mov, invoke_code, WORD


def mov_reg(rd, rn):
    return [0xAA000000 | (rn << 5) | rd]

CODE, DATA, STACK = 0x1000000, 0x1100000, 0x1200000
PAGE = 4096
IPC_VA = 0x8000000 - PAGE  # managed children's IPC buffer (USER_ADDRESS_LIMIT - PAGE)

CALL, NBRECV = WORD, WORD - 7
SVC = 0xd4000001
IRQ_GET, IRQ_ACK, IRQ_SET, IRQ_CLEAR = 26, 27, 28, 29
INVALID_ARGUMENT, INVALID_CAPABILITY = 1, 2
RANGE_ERROR, TRUNCATED_MESSAGE = 4, 7
NOT_FOUND, ALREADY_MAPPED, REVOKE_FIRST = 6, 8, 9

NTFN, NTFN_BADGED, HANDLER = 121, 122, 130
IRQ_BADGE = 0x40

PLATFORM = Path(__file__).resolve().parents[1] / 'target/platform/qemu-arm-virt/platform.json'


class IrqClient(Client):
    """Adds raw (non-blocking) syscall execution to the root task."""

    def sysc(self, cap, syscall, tag=0, x2=0):
        g = self.gdb
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


def phy_write(g, address, data):
    assert g.command('Qqemu.PhyMemMode:1') == 'OK'
    try:
        write(g, address, data)
    finally:
        assert g.command('Qqemu.PhyMemMode:0') == 'OK'


def phy_word(g, address):
    assert g.command('Qqemu.PhyMemMode:1') == 'OK'
    try:
        return g.word(address)
    finally:
        assert g.command('Qqemu.PhyMemMode:0') == 'OK'


class Irq:
    """GICD register access for one INTID (distributor registers only)."""

    def __init__(self, g, gicd, intid):
        self.g, self.gicd, self.intid = g, gicd, intid
        self.offset = 4 * (intid // 32)
        self.bit = 1 << (intid % 32)

    def _read(self, base):
        return phy_word(self.g, self.gicd + base + self.offset)

    def _write(self, base, register):
        phy_write(self.g, self.gicd + base + self.offset,
                  struct.pack('<I', register | self.bit))

    def pend(self):
        self._write(0x200, 0)  # ISPENDR: latch pending in software

    @property
    def enabled(self):
        return bool(self._read(0x100) & self.bit)  # ISENABLER

    @property
    def active(self):
        return bool(self._read(0x300) & self.bit)  # ISACTVR


def run(qemu, kernel):
    info = json.loads(PLATFORM.read_text())
    gicd = info['GICD_BASE']
    # The generated table lists VirtIO slot lines first (kind 0), then other
    # device lines; the tests exercise the first two slot lines.
    virtio = [line for line in info['irq_lines'] if line[2] == 0]
    line = Irq(None, gicd, virtio[0][0])  # rebound per run
    directory = boot_image(kernel).parent
    image = parse_elf((directory / 'userboot').read_bytes())
    ipc = root_layout(image, (directory / 'kernel.dtb').stat().st_size)['ipc']
    with tempfile.TemporaryDirectory(prefix='rstiny-irq-') as temporary:
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
            c = IrqClient(g, image['entry'], ipc)
            line.g = g
            spare = Irq(g, gicd, virtio[1][0])
            baseline = c.runtime('available')
            children = []

            def retype(kind, slot, bits=0, count=1, alloc=32, status=0):
                return c.call(alloc, 1, [kind, bits, 0, 0, slot, count], [2], status)

            def mint(dst, src, badge=0, rights=15, status=0):
                return c.call(2, 21, [dst, 64, src, 64, rights, badge], [2], status)

            def delete(slot, status=0):
                c.call(2, 18, [slot, 64], status=status)

            def child(words):
                """A managed task with a code page (see check_ipc.py)."""
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

            def wait_bits(bits, code):
                for _ in range(2000):
                    badge, _, _ = c.nbrecv(NTFN)
                    if badge == bits:
                        return
                    c.runtime('sleep', 1)
                raise AssertionError(('no delivery', code, bits))

            def no_delivery(code):
                for _ in range(40):
                    badge, _, _ = c.nbrecv(NTFN)
                    assert badge == 0, ('unexpected delivery', code, badge)
                    c.runtime('sleep', 1)

            # The platform table rejects every non-VirtIO line: the timer PPI,
            # SGIs, past the window and the special-ID range (docs/irq.md §3.1).
            timer = Irq(g, gicd, info['timer_irq'])
            last = info['irq_lines'][-1][0]
            for intid in (info['timer_irq'], 0, last + 1, 1020):
                c.call(4, IRQ_GET, [intid, HANDLER, 64], [2], status=INVALID_ARGUMENT)
            c.call(4, IRQ_GET, [line.intid, HANDLER, 32], [2], status=INVALID_ARGUMENT)
            c.call(4, IRQ_GET, [line.intid, 0, 64], [2], status=RANGE_ERROR)
            c.call(4, IRQ_GET, [line.intid, HANDLER, 64], status=TRUNCATED_MESSAGE)

            # A valid Get authorizes the line and enables it for delivery.
            retype(3, NTFN)
            mint(NTFN_BADGED, NTFN, badge=IRQ_BADGE)
            c.call(4, IRQ_GET, [line.intid, HANDLER, 64], [2])
            assert line.enabled, 'Get must enable the line'
            c.call(4, IRQ_GET, [line.intid, HANDLER + 1, 64], [2], status=REVOKE_FIRST)
            c.call(4, IRQ_GET, [spare.intid, HANDLER, 64], [2], status=ALREADY_MAPPED)

            # SetNotification validates the capability kind.
            c.call(HANDLER, IRQ_SET, [], [2], status=INVALID_CAPABILITY)
            c.call(HANDLER, IRQ_SET, [], [NTFN_BADGED])

            # Delivery: software-pend the line, the bound Notification merges
            # the binding badge into its pending bits.
            line.pend()
            wait_bits(IRQ_BADGE, 'badge delivery')
            assert line.active, 'delivered line must stay active (no deactivate)'

            # The active state masks re-delivery: an uncleared source cannot
            # re-enter the kernel until the driver acknowledges (docs/irq.md §5).
            line.pend()
            no_delivery('active line must not redeliver')

            # Ack deactivates; the latched pending fires again immediately.
            # (The redelivery itself proves the deactivation happened: while
            # active, the line could not fire — as the phase above showed.)
            c.call(HANDLER, IRQ_ACK)
            wait_bits(IRQ_BADGE, 'redelivery after Ack')
            assert line.active, 'redelivered line is active again'
            c.call(HANDLER, IRQ_ACK)

            # Clear unbinds and quiesces the line: enabled=0, silent when pended.
            c.call(HANDLER, IRQ_CLEAR)
            assert not line.enabled, 'Clear must disable the line'
            line.pend()
            no_delivery('cleared line must stay silent')

            # Rebinding recovers a disabled line (the restart path) and the
            # latched pending delivers as soon as the line is live again.
            c.call(HANDLER, IRQ_SET, [], [NTFN_BADGED])
            assert line.enabled, 'rebind must re-enable the line'
            wait_bits(IRQ_BADGE, 'redelivery after rebind')
            c.call(HANDLER, IRQ_ACK)

            # A managed child has no IRQControl: invoking the (empty) slot 4
            # is NOT_FOUND (docs/irq.md §8).
            stranger = child(mov(0, 4) + mov(1, 0) + mov(7, CALL) + [SVC] +
                             mov_reg(2, 1) + invoke_code('exit', [('reg', 2)]))
            start(stranger)
            assert c.runtime('wait', stranger) >> 12 == NOT_FOUND, 'child must have no IRQControl'

            # Cleanup: unbind first so the notification object is reclaimable.
            c.call(HANDLER, IRQ_CLEAR)
            for handle in children:
                c.runtime('destroy', handle)
            delete(NTFN_BADGED)
            delete(NTFN)
            assert c.runtime('available') == baseline, 'IRQ test leaked memory'
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
    modes = os.environ.get('IRQ_MODES', 'debug,release').split(',')
    levels = os.environ.get('IRQ_LEVELS', 'off,info').split(',')
    for mode in modes:
        for level in levels:
            kernel = build(mode, level, False)
            print(f'CHECK irq {mode} LOG={level}', flush=True)
            run(args.qemu, kernel)
    print('PASS: platform-table validation; duplicate Get; badge delivery; '
          'active-state storm protection; Ack redelivery; Clear quiescing and '
          'rebind recovery; children hold no IRQControl.', flush=True)


if __name__ == '__main__':
    main()
