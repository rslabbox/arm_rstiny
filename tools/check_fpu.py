#!/usr/bin/env python3
"""Exercise lazy EL0 FP/SIMD enablement, isolation and ownership migration.

Each scenario boots a fresh machine with the kernel image built by the Makefile:

- numerics: a task computes 2.0*2.0*2.0*2.0 in d-registers and returns the
  exact IEEE-754 double bits. Before docs/fpu.md this first FP instruction
  faulted with ESR EC 0x07, so this scenario is the enablement proof.
- fresh-task zero state: FPCR/FPSR read as zero at EL0; d0 of a brand-new
  task is 0.0 even when a previous owner left nonzero data in the hardware.
- owner handover: an FP owner sleeps while a second task claims the hardware
  and exits; the first resumes and still sees its own value, proving the
  migrate/unmigrate save-restore round trip (docs/fpu.md section 8).
- control-register persistence: FZ (FPCR) and the IXC/C flags (FPSR) set by
  an inexact divide survive ownership handover; most incomplete lazy FPU
  designs forget these two system registers.
- destroy while owner: reclaiming an FP-owning task must clear ownership so a
  later migrate never writes into the freed context; the next FP user still
  sees all-zero state.
- UDF #0 still faults (EC 0x00); FP is handled by the kernel, not by halting
  the task, so genuine undefined instructions stay isolated.
"""
import argparse
import struct
import subprocess
import tempfile
from pathlib import Path
from check_kernel import Gdb, build, boot_image, mappings
from check_fatboot import write
from elf_image import parse_elf, root_layout
from abi_client import Client, mov, invoke_code

CODE, DATA, STACK = 0x1000000, 0x1100000, 0x1200000

# Encodings produced by aarch64-linux-gnu-as (see docs/fpu.md section 12).
# `exit`/`wait` pass the task result through MR0 (x2), so results land in x2.
FMOV_D0_X0, FMOV_D1_X0 = 0x9e670000, 0x9e670001  # fmov d0/d1, x0
FMOV_D0_X2 = 0x9e670040  # fmov d0, x2
FMOV_X2_D0 = 0x9e660002  # fmov x2, d0
FMUL_D0_D0_D1 = 0x1e610800  # fmul d0, d0, d1
FDIV_D0_D0_D1 = 0x1e611800  # fdiv d0, d0, d1
MRS_X2_FPSR, MRS_X3_FPCR = 0xd53b4422, 0xd53b4403  # mrs x2, fpsr / x3, fpcr
MSR_FPCR_X0 = 0xd51b4400  # msr fpcr, x0
ORR_X2_X3 = 0xaa030042  # orr x2, x2, x3

TWO, SIXTEEN = 0x4000000000000000, 0x4030000000000000
FIVE, SEVEN = 0x4014000000000000, 0x401c000000000000
ONE, THREE = 0x3ff0000000000000, 0x4008000000000000
FZ = 0x01000000  # FPCR.FZ (flush-to-zero); 1/3 is inexact -> FPSR.IXC (bit 4)
TASK_SLEEPING, TASK_FAULTED = 5, 3


def run(qemu, kernel):
    directory = boot_image(kernel).parent
    image = parse_elf((directory / 'userboot').read_bytes())
    layout = root_layout(image, (directory / 'kernel.dtb').stat().st_size)
    entry, buffer = image['entry'], layout['ipc']
    with tempfile.TemporaryDirectory(prefix='rstiny-fpu-') as temporary:
        tmp = Path(temporary)
        serial = tmp / 'serial'
        proc = subprocess.Popen([
            qemu, '-machine', 'virt,gic-version=3,virtualization=off', '-cpu', 'cortex-a72',
            '-smp', '1', '-m', '128M', '-display', 'none', '-monitor', 'none', '-nic', 'none',
            '-serial', f'file:{serial}', '-kernel', str(boot_image(kernel)),
            '-S', '-gdb', f'unix:{tmp / "gdb"},server=on,wait=off',
        ], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        gdb = None
        try:
            gdb = Gdb(tmp / 'gdb', proc)
            gdb.run_to(entry)
            client = Client(gdb, entry, buffer)
            call = client.runtime
            baseline = call('available')

            def task(code):
                handle = call('create')
                for address in (CODE, DATA, STACK):
                    call('map', handle, address, 4096, 3)
                write(gdb, buffer, struct.pack('<'+'I'*len(code), *code))
                call('write', handle, CODE, buffer, 4*len(code))
                call('protect', handle, CODE, 4096, 5)
                return handle

            def start(handle):
                call('start', handle, CODE, STACK + 4096, 0)

            def destroy(handle):
                call('destroy', handle)

            def wait_state(handle, state):
                for _ in range(40):
                    if call('status', handle) == state:
                        return
                    call('sleep', 1)
                raise AssertionError(('task did not reach state', handle, state))

            # 1) FP/SIMD enabled and arithmetic correct: (2.0)^2 * 2.0 * 2.0
            # = 16.0. Pre-FPU this task faulted on its first fmov with ESR
            # EC 0x07.
            compute = task(mov(0, TWO) + [FMOV_D0_X0, FMOV_D1_X0,
                            FMUL_D0_D0_D1, FMUL_D0_D0_D1, FMUL_D0_D0_D1,
                            FMOV_X2_D0]
                           + invoke_code('exit', [('reg', 2)]))
            start(compute)
            assert call('wait', compute) == SIXTEEN
            destroy(compute)

            # 2) Fresh task sees all-zero FPCR/FPSR (and never faults reading
            # them: the read itself only works while the CPACR window is open).
            fresh = task([MRS_X2_FPSR, MRS_X3_FPCR, ORR_X2_X3]
                         + invoke_code('exit', [('reg', 2)]))
            start(fresh)
            assert call('wait', fresh) == 0
            destroy(fresh)

            # 3) Owner handover: A claims FP (d0 = 5.0) and sleeps; B claims
            # in the meantime (d0 = 7.0) and exits; A resumes and still reads
            # its own 5.0. This is the full A -> B -> A migrate round trip.
            a = task(mov(0, FIVE) + [FMOV_D0_X0] + invoke_code('sleep', [300])
                     + [FMOV_X2_D0] + invoke_code('exit', [('reg', 2)]))
            b = task(mov(2, SEVEN) + [FMOV_D0_X2, FMOV_X2_D0]
                     + invoke_code('exit', [('reg', 2)]))
            start(a)
            call('sleep', 50)  # A traps on fmov, then sleeps as owner.
            start(b)
            assert call('wait', b) == SEVEN
            assert call('wait', a) == FIVE
            destroy(a)
            destroy(b)

            # 4) FPCR/FPSR persist across ownership: FZ stays set and the
            # inexact-flag value built before sleep survives B's claim/exit.
            control = task(mov(0, FZ) + [MSR_FPCR_X0]
                           + mov(0, ONE) + [FMOV_D0_X0]
                           + mov(0, THREE) + [FMOV_D1_X0]
                           + [FDIV_D0_D0_D1]  # 1/3: inexact -> IXC and C set
                           + invoke_code('sleep', [200])
                           + [MRS_X2_FPSR, MRS_X3_FPCR, ORR_X2_X3]
                           + invoke_code('exit', [('reg', 2)]))
            claim = task(mov(2, TWO) + [FMOV_D0_X2, FMOV_X2_D0]
                         + invoke_code('exit', [('reg', 2)]))
            start(control)
            call('sleep', 50)
            start(claim)
            assert call('wait', claim) == TWO
            result = call('wait', control)
            assert result & FZ == FZ, hex(result)  # FPCR.FZ survived handover
            assert result & 0x10 == 0x10, hex(result)  # FPSR.IXC also survived
            destroy(control)
            destroy(claim)

            # 5) Destroy an FP owner while it sleeps: forget clears the bond,
            # so the next FP user starts from zero instead of inheriting the
            # reclaimed task's stale hardware registers.
            owner = task(mov(0, FIVE) + [FMOV_D0_X0] + invoke_code('sleep', [300])
                         + [FMOV_X2_D0] + invoke_code('exit', [('reg', 2)]))
            start(owner)
            wait_state(owner, TASK_SLEEPING)  # owner is now the FPU owner
            destroy(owner)
            successor = task([FMOV_X2_D0] + invoke_code('exit', [('reg', 2)]))
            start(successor)
            assert call('wait', successor) == 0
            destroy(successor)

            # 6) Real undefined instructions still fault (EC 0x00); FP access
            # is handled, not punished, by the kernel.
            fault = task([0x00000000])
            start(fault)
            esr = call('wait', fault)
            assert esr >> 26 == 0, hex(esr)  # EC 0x00 (undefined), FP handled otherwise
            assert call('status', fault) == TASK_FAULTED
            destroy(fault)

            assert mappings(gdb) and call('available') == baseline
            assert proc.poll() is None
            print('    fpu: numerics fresh-state handover control-regs destroy-owner ubefault OK',
                  flush=True)
        except Exception:
            print(serial.read_text(errors='replace'), flush=True)
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
    kernel = build('release', 'info', False)
    run(args.qemu, kernel)


if __name__ == '__main__':
    main()