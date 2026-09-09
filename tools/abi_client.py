"""AArch64 seL4 non-MCS wire client used by real-EL0 tests."""
import struct
from check_fatboot import write

WORD = (1 << 64) - 1
CALL, YIELD, DEBUG_PUTCHAR = WORD, WORD - 6, WORD - 8
RUNTIME = dict(current=0x1000, create=0x1001, start=0x1002, status=0x1003,
               destroy=0x1004, wait=0x1005, sleep=0x1006, exit=0x1007,
               clock=0x1008, available=0x1009, map=0x100a, unmap=0x100b,
               protect=0x100c, write=0x100d, read=0x100e, empty=0x100f,
               debug_available=0x1010, cspace=0x1011, vspace=0x1012)

def mov(register, value):
    value &= WORD
    result = [0xd2800000 | ((value & 65535) << 5) | register]
    for shift in range(1, 4):
        part = (value >> (shift * 16)) & 65535
        if part:
            result.append(0xf2800000 | (shift << 21) | (part << 5) | register)
    return result

def invoke_code(method, args=(), cap=17):
    """Arguments may be constants or ('reg', n); result is MR0/x2."""
    label = RUNTIME[method] if isinstance(method, str) else method
    code = []
    # Test programs only use a register source when it is already in its MR.
    for index, arg in enumerate(args):
        dest = index + 2
        if isinstance(arg, tuple):
            assert arg == ('reg', dest)
        else:
            code += mov(dest, arg)
    return code + mov(0, cap) + mov(1, (label << 12) | len(args)) + mov(7, CALL) + [0xd4000001]

class Client:
    def __init__(self, gdb, entry, ipc):
        self.gdb, self.entry, self.ipc = gdb, entry, ipc
        write(gdb, entry, struct.pack('<I', 0xd4000001))

    def call(self, cap, label, args=(), caps=(), status=0):
        g = self.gdb
        for index, value in enumerate(args[4:], 4):
            write(g, self.ipc + 8 + index * 8, struct.pack('<Q', value & WORD))
        for index, value in enumerate(caps):
            write(g, self.ipc + 976 + index * 8, struct.pack('<Q', value))
        g.write_reg('x0', cap)
        g.write_reg('x1', (label << 12) | (len(caps) << 7) | len(args))
        for index in range(4):
            g.write_reg(f'x{index+2}', (args[index] if index < len(args) else 0) & WORD)
        g.write_reg('x7', CALL)
        g.write_reg('pc', self.entry)
        g.run_to(self.entry + 4)
        actual = g.reg('x1') >> 12
        assert actual == status, (cap, label, args, actual, status)
        assert g.reg('x0') == 0 and g.reg('cpsr') & 15 == 0
        return g.reg('x2')

    def runtime(self, name, *args, status=0):
        return self.call(17, RUNTIME[name], args, status=status)

    def suspend(self, cap):
        return self.call(cap, 11)

    def resume(self, cap, status=0):
        return self.call(cap, 12, status=status)

    def copy(self, destination_cnode, slot, source_slot, rights=15, source_cnode=2):
        return self.call(destination_cnode, 20, [slot,64,source_slot,64,rights], [source_cnode])
