#!/usr/bin/env python3
"""Exercise standard object invocation, rights, CSpaces and revoke on real EL0."""
import argparse
from pathlib import Path
import struct
import subprocess
import tempfile
from check_kernel import Gdb, build, boot_image
from check_fatboot import write, pages
from elf_image import parse_elf, root_layout
from abi_client import Client, invoke_code

def run(qemu,kernel):
    directory = boot_image(kernel).parent
    image = parse_elf((directory/'userboot').read_bytes())
    ipc = root_layout(image,(directory/'kernel.dtb').stat().st_size)['ipc']
    with tempfile.TemporaryDirectory(prefix='rstiny-caps-') as temporary:
        tmp = Path(temporary)
        proc = subprocess.Popen([
            qemu,'-machine','virt,gic-version=3,virtualization=off','-cpu','cortex-a72',
            '-smp','1','-m','128M','-display','none','-monitor','none','-nic','none',
            '-serial',f'file:{tmp / "serial"}','-kernel',str(boot_image(kernel)),
            '-S','-gdb',f'unix:{tmp / "gdb"},server=on,wait=off',
        ],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        g = None
        try:
            g = Gdb(tmp/'gdb',proc); g.run_to(image['entry'])
            c = Client(g,image['entry'],ipc)
            before = c.runtime('available')
            def retype(kind,slot,count=1,bits=0,status=0):
                return c.call(32,1,[kind,bits,0,0,slot,count],[2],status)
            def delete(slot): c.call(2,18,[slot,64])
            def revoke(slot): c.call(2,17,[slot,64])

            # Correct IPC-buffer marshalling for a six-word request plus one cap.
            retype(7,200)
            retype(7,200,status=8)
            # Endpoint and Notification are real metadata objects now; badge
            # mint ANDs and IPC delivery are covered by check_ipc.py.
            retype(2,220)
            retype(3,221)
            c.call(2,21,[222,64,220,64,15,0x70],[2])
            c.call(2,21,[223,64,222,64,15,0x0f],[2],status=3) # badged source refuses mutation
            revoke(222)
            delete(222)
            delete(220)
            delete(221)
            c.call(32,1,[7,0,0,0,201],[2],status=7)
            c.call(200,11,status=3) # a frame is not a TCB
            c.call(2,18,[200,32],status=6)

            scratch = 0x07000000
            retype(9,201)
            c.call(200,40,[scratch,3,5],[3],status=6) # no L3 table
            c.call(201,38,[scratch,1],[3])
            c.copy(2,209,201)
            c.call(209,38,[scratch+0x200000,1],[3],status=1)
            delete(209)
            c.call(200,40,[scratch+1,3,5],[3],status=5)
            c.call(200,40,[scratch,3,5],[3])
            physical = c.call(200,46)
            c.copy(2,202,200,rights=2)
            c.call(202,40,[scratch+4096,3,5],[3])
            mapping = pages(g)
            assert mapping[scratch][0] == mapping[scratch+4096][0] == physical
            assert not mapping[scratch][1] & (1 << 7)
            assert mapping[scratch+4096][1] & (1 << 7), 'copy amplified frame write rights'
            write(g,scratch,b'CAPS')
            assert g.memory(scratch+4096,4) == b'CAPS'
            c.call(200,40,[scratch+8192,3,5],[3],status=1) # one mapping per frame cap
            c.copy(2,203,202,rights=15)
            c.call(203,40,[scratch+8192,3,5],[3])
            assert pages(g)[scratch+8192][1] & (1 << 7), 'descendant regained write'
            revoke(202)
            c.call(203,46,status=6)
            assert scratch+8192 not in pages(g)
            c.call(200,41)
            delete(200) # original cap deletion does not revoke independent descendants
            assert c.call(202,46) == physical
            c.call(202,41); delete(202); delete(201)
            revoke(32) # region-granular reclamation resets the Untyped
            assert c.runtime('available') == before

            # A failed revoke must forget mappings it already removed. Reusing
            # that VA must not let the old cap remove an unrelated new mapping.
            c.copy(2,210,32)
            c.call(210,1,[7,0,0,0,211,1],[2])
            c.call(210,1,[9,0,0,0,212,1],[2])
            retype(7,213); retype(7,214)
            c.call(212,38,[scratch,1],[3])
            c.call(211,40,[scratch,3,5],[3])
            c.call(213,40,[scratch+4096,3,5],[3])
            c.call(2,17,[210,64],status=3) # unrelated frame keeps L3 nonempty
            assert scratch not in pages(g)
            c.call(214,40,[scratch,3,5],[3])
            delete(211) # already unmapped: must leave cap 214's mapping intact
            assert pages(g)[scratch][0] == c.call(214,46)
            c.call(213,41); c.call(214,41)
            revoke(210); delete(210); delete(213); delete(214)
            assert c.runtime('available') == before

            # Managed unmap may remove a standard frame mapping. Its old cap
            # must neither change nor remove a later mapping at the same VA.
            retype(7,220,2); retype(9,222)
            c.call(222,38,[scratch,1],[3])
            c.call(220,40,[scratch,3,5],[3])
            c.runtime('unmap',1,scratch,4096)
            c.call(221,40,[scratch,3,5],[3])
            c.call(220,40,[scratch,2,5],[3],status=2)
            c.call(220,41)
            assert pages(g)[scratch][0] == c.call(221,46)
            delete(220); c.call(221,41); delete(221); delete(222)
            revoke(32) # region-granular reclamation resets the Untyped
            assert c.runtime('available') == before

            # Retype and configure a task using standard TCB/VSpace/Page/CNode
            # methods; only exit/wait use the explicit runtime extension.
            retype(6,100) # VSpace
            retype(9,101) # L3
            retype(4,102,bits=16) # flat guarded CNode
            retype(1,103) # TCB
            retype(7,104,3) # code, IPC, stack
            retype(9,110) # root scratch L3
            c.call(101,38,[0x1000000,1],[100],status=2) # requires ASID assignment
            c.call(6,48,caps=[100])
            c.call(6,48,caps=[100],status=2)
            c.call(101,38,[0x1000000,1],[100])
            c.call(110,38,[scratch,1],[3])
            c.copy(2,111,104)
            c.call(111,40,[scratch,3,5],[3])
            code = invoke_code('exit',[202])
            write(g,scratch,struct.pack('<'+'I'*len(code),*code))
            c.call(111,41); delete(111); delete(110)
            c.call(104,40,[0x1000000,2,1],[100]) # RX
            c.call(105,40,[0x1001000,3,5],[100]) # IPC
            c.call(106,40,[0x1002000,3,5],[100]) # stack
            for dest,source in [(1,103),(2,102),(3,100),(17,17)]:
                c.copy(102,dest,source)
            c.call(103,5,[0,48,0,0x1001000],[102,100,106],status=2) # wrong IPC frame
            c.call(103,5,[0,48,0,0x1001000],[102,100,105])
            c.call(103,3,[0,4,0x1000000,0x1003000,5,0],status=1) # EL1 SPSR rejected
            c.call(103,3,[0,4,0x1000000,0x1003000,0,0])
            c.call(103,12)
            assert c.runtime('wait',103) == 202
            assert c.runtime('status',103) == 6
            # Revoke all descendants of the allocator capability. TCB, CNode,
            # frame aliases and page-table references must all be reclaimed.
            revoke(32)
            assert c.runtime('available') == before
            c.call(103,12,status=6)

            # Retype allocation failure is atomic: no prefix of a batch leaks.
            slot = 1000
            while True:
                label, _ = c.raw(32, 1, [7,0,0,0,slot,32], [2])
                if label != 0:
                    assert label == 10, label
                    available = c.runtime('available')
                    label, _ = c.raw(32, 1, [7,0,0,0,slot,32], [2])
                    assert label == 10
                    assert c.runtime('available') == available
                    c.call(slot,46,status=6)
                    break
                slot += 32
            revoke(32)
            assert c.runtime('available') == before
        except Exception:
            print((tmp/'serial').read_text(errors='replace'),flush=True)
            raise
        finally:
            if g: g.sock.close()
            proc.terminate(); proc.wait(timeout=5)

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--qemu',default='qemu-system-aarch64')
    args=parser.parse_args()
    for mode in ('debug','release'):
        for level in ('off','info'):
            kernel=build(mode,level,False)
            print(f'CHECK object capabilities {mode} LOG={level}',flush=True)
            run(args.qemu,kernel)
    print('PASS: seL4 Call marshalling; Retype/TCB/CNode/VSpace/frame methods; ASID gating; alias rights; revoke; atomic exhaustion.',flush=True)
if __name__ == '__main__': main()
