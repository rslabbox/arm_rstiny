#!/usr/bin/env python3
"""Exercise the capability-authorized managed runtime on real EL0 tasks."""
import argparse
from pathlib import Path
import struct
import subprocess
import tempfile
from check_kernel import Gdb, build, boot_image, mappings
from check_fatboot import write
from elf_image import parse_elf, root_layout
from abi_client import Client, mov, invoke_code

CODE, DATA, STACK = 0x1000000, 0x1100000, 0x1200000
PAGE = 4096

def run(qemu, kernel):
    directory = boot_image(kernel).parent
    image = parse_elf((directory / 'userboot').read_bytes())
    layout = root_layout(image, (directory / 'kernel.dtb').stat().st_size)
    entry, buffer = image['entry'], layout['ipc']
    with tempfile.TemporaryDirectory(prefix='rstiny-tasks-') as temporary:
        tmp = Path(temporary)
        serial = tmp / 'serial'
        proc = subprocess.Popen([
            qemu, '-machine', 'virt,gic-version=3,virtualization=off', '-cpu','cortex-a72',
            '-smp','1','-m','128M','-display','none','-monitor','none','-nic','none',
            '-serial',f'file:{serial}','-kernel',str(boot_image(kernel)),
            '-S','-gdb',f'unix:{tmp / "gdb"},server=on,wait=off',
        ], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        gdb = None
        try:
            gdb = Gdb(tmp / 'gdb', proc)
            gdb.run_to(entry)
            client = Client(gdb, entry, buffer)
            call = client.runtime
            assert call('current') == 1
            root = 1
            baseline = call('available')
            child = call('create')
            assert call('available') == baseline - 5 # VSpace plus IPC mapping
            call('map', 65535, DATA, PAGE, 3, status=6)
            for address,length,rights in [(0,PAGE,3),(DATA+1,PAGE,3),(DATA,0,3),
                                          (DATA,PAGE,7),(0x8000000,PAGE,3),(2**64-4096,PAGE,3)]:
                call('map',child,address,length,rights,status=1)
            call('map',child,DATA,2*PAGE,3)
            free = call('available')
            call('map',child,DATA,PAGE,3,status=8)
            assert call('available') == free
            payload = bytes(range(32))
            write(gdb, buffer, payload)
            call('write',child,DATA+PAGE-16,buffer,32)
            call('read',child,DATA+PAGE-16,buffer+128,32)
            assert gdb.memory(buffer+128,32) == payload
            call('protect',child,DATA+PAGE,PAGE,1)
            write(gdb,buffer,bytes([255])*32)
            call('write',child,DATA+PAGE-16,buffer,32,status=3)
            call('read',child,DATA+PAGE-16,buffer+128,32)
            assert gdb.memory(buffer+128,32) == payload
            for invalid in (0,0x40000000,2**64-8):
                for method in ('read','write'):
                    call(method,child,DATA,invalid,32,status=1)
                    call(method,child,invalid,buffer,32,status=1)
            call('write',child,DATA,buffer,4097,status=1)
            call('read',child,DATA,entry,8,status=3)
            call('unmap',child,DATA,3*PAGE,status=6)
            for pinned in (layout['boot_info'],layout['extra']):
                call('unmap',root,pinned,PAGE,status=3)
                call('protect',root,pinned,PAGE,3,status=3)
            for method in ('read','write'):
                write(gdb,buffer,payload+bytes(16))
                args = (buffer+8,buffer) if method == 'write' else (buffer,buffer+8)
                call(method,root,*args,32)
                assert gdb.memory(buffer+8,32) == payload
            call('unmap',child,DATA,2*PAGE)
            call('map',child,DATA,PAGE,3)
            call('read',child,DATA,buffer,64)
            assert gdb.memory(buffer,64) == bytes(64)
            call('destroy',child)
            assert call('available') == baseline
            call('status',child,status=6)

            # Quota/exhaustion rollback, including the managed IPC page. The
            # managed region is large enough for three 1023-page tasks but not a
            # fourth, so the failed mapping must be atomic.
            tasks = [call('create') for _ in range(4)]
            for task in tasks[:3]:
                call('map',task,CODE,1023*PAGE,3)
            before = call('available')
            call('map',tasks[3],CODE,1023*PAGE,3,status=10)
            assert call('available') == before
            call('read',tasks[3],CODE,buffer,8,status=6)
            call('map',tasks[3],CODE,1024*PAGE,3,status=10)
            for task in tasks: call('destroy',task)
            assert call('available') == baseline

            def task(code):
                handle = call('create')
                for address in (CODE,DATA,STACK): call('map',handle,address,PAGE,3)
                write(gdb,buffer,struct.pack('<'+'I'*len(code),*code))
                call('write',handle,CODE,buffer,4*len(code))
                call('protect',handle,CODE,PAGE,5)
                return handle
            def start(handle):
                call('start',handle,CODE,STACK+PAGE,0)
            def wait_state(handle, state):
                for _ in range(40):
                    if call('status',handle) == state: return
                    call('sleep',1)
                raise AssertionError(('task did not reach state',handle,state))

            # Only hardware timer IRQs can regain control from either worker.
            spin = mov(9,DATA)+[0xf940012a,0x9100054a,0xf900012a,0x17fffffd]
            a,b = task(spin),task(spin)
            call('start',a,DATA,STACK+PAGE,0,status=3)
            call('start',a,CODE,STACK+PAGE-1,0,status=1)
            client.resume(a,status=3)
            start(a); start(b)
            call('map',a,DATA+PAGE,PAGE,3,status=3)
            call('sleep',30)
            client.suspend(a); client.suspend(b)
            call('read',a,DATA,buffer,8); count_a = gdb.word(buffer)
            call('read',b,DATA,buffer,8); count_b = gdb.word(buffer)
            assert count_a > 0 and count_b > 0
            write(gdb,buffer,struct.pack('<Q',0xfeed))
            call('write',a,DATA,buffer,8)
            call('read',b,DATA,buffer,8)
            assert gdb.word(buffer) == count_b
            before = call('clock'); call('sleep',25)
            assert call('clock')-before >= 25
            client.resume(a); call('sleep',20); client.suspend(a)
            call('read',a,DATA,buffer,8)
            assert gdb.word(buffer) not in (0,0xfeed)
            call('destroy',a); client.resume(b); call('destroy',b)
            assert call('available') == baseline

            sleeper = task(invoke_code('sleep',[100])+invoke_code('exit',[43]))
            before = call('clock'); start(sleeper); client.suspend(sleeper)
            assert call('status',sleeper) == 2
            client.resume(sleeper)
            assert call('wait',sleeper) == 43 and call('clock')-before >= 100
            call('destroy',sleeper)
            assert call('available') == baseline

            # Cross-CSpace authority must be granted explicitly. The waiter gets
            # a copy of its sibling's TCB cap; no global task ID is accepted.
            target = task(invoke_code('sleep',[200])+invoke_code('exit',[42]))
            waiter = task(invoke_code('wait',[32])+invoke_code('exit',[('reg',2)]))
            node = call('cspace',waiter)
            client.copy(node,32,target)
            start(target); start(waiter); wait_state(waiter,7)
            client.suspend(waiter); call('sleep',250)
            assert call('status',waiter) == 2
            client.resume(waiter)
            assert call('wait',waiter) == 42
            call('destroy',waiter); call('destroy',target)
            assert call('available') == baseline

            # A number naming a cap in root's CSpace has no authority in a child.
            victim = call('create')
            denied = task(invoke_code('status',[victim])+[0xd34cfc22]+invoke_code('exit',[('reg',2)]))
            start(denied)
            assert call('wait',denied) == 6
            call('destroy',denied); call('destroy',victim)

            # EC 0x00 is any other unallocated instruction: FP/SIMD is no
            # longer one of them (docs/fpu.md), so the third probe is a plain
            # UDF #0; FP behavior moved to check_fpu.py.
            for code,ec in [(mov(9,0)+[0xf9400120],0x24),([0xd51be220],0x18),([0x00000000],0x00)]:
                fault = task(code)
                start(fault)
                assert call('wait',fault) >> 26 == ec
                assert call('status',fault) == 3
                call('destroy',fault)
                assert call('available') == baseline

            before_stacks = mappings(gdb)
            sleepers = [task(invoke_code('sleep',[60000])+[0x14000000]) for _ in range(31)]
            before_failure = call('available')
            call('create',status=10)
            assert call('available') == before_failure
            for sleeper in sleepers:
                start(sleeper); wait_state(sleeper,5)
            assert len(before_stacks.keys()-mappings(gdb).keys()) == 62
            for sleeper in sleepers: call('destroy',sleeper)
            assert mappings(gdb) == before_stacks and call('available') == baseline
            for _ in range(260):
                exited = task(invoke_code('exit',[42]))
                start(exited)
                assert call('wait',exited) == 42
                call('destroy',exited)
            assert mappings(gdb) == before_stacks and call('available') == baseline
            assert proc.poll() is None
        except Exception:
            print(serial.read_text(errors='replace'),flush=True)
            raise
        finally:
            if gdb: gdb.sock.close()
            proc.terminate(); proc.wait(timeout=5)

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--qemu',default='qemu-system-aarch64')
    args = parser.parse_args()
    for mode in ('debug','release'):
        for level in ('off','info'):
            kernel = build(mode,level,False)
            print(f'CHECK capability runtime {mode} LOG={level}',flush=True)
            run(args.qemu,kernel)
    print('PASS: capability scope/transfer; memory rollback/recycling; timer preemption; suspended wait completion; faults; guarded stack reclamation.',flush=True)

if __name__ == '__main__': main()
