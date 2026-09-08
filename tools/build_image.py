#!/usr/bin/env python3
"""Build the unmodified seL4 ARM elfloader and its kernel/DTB/rootserver CPIO."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
from elf_image import parse_elf, validate_pair

ROOT = Path(__file__).resolve().parents[1]
LOADER = ROOT / 'loader'


def command(args, **kwargs):
    subprocess.run([str(arg) for arg in args], check=True, **kwargs)


def build(kernel, root, output, qemu='qemu-system-aarch64', cc='aarch64-linux-gnu-gcc'):
    manifest = json.loads((LOADER / 'upstream.json').read_text())
    for name, checksum in manifest['sha256'].items():
        if hashlib.sha256((LOADER / 'vendor' / name).read_bytes()).hexdigest() != checksum:
            raise ValueError(f'upstream loader source changed: {name}')
    kernel_info, root_info = parse_elf(kernel.read_bytes()), parse_elf(root.read_bytes())
    output.mkdir(parents=True, exist_ok=True)
    sources = [LOADER / name for name in json.loads((LOADER / 'sources.json').read_text())]
    includes = [LOADER / 'config', output, LOADER / 'vendor/libcpio/include']
    inc = LOADER / 'vendor/elfloader/include'
    includes += [inc / path for path in ('', 'plat/qemu-arm-virt', 'arch-arm', 'arch-arm/64',
                                        'arch-arm/armv/armv8-a', 'arch-arm/armv/armv8-a/64')]
    flags = ['-march=armv8-a', '-D__KERNEL_64__', '-D_XOPEN_SOURCE=700', '-O2', '-g',
             '-ffreestanding', '-Wall', '-Werror', '-Wextra', '-mgeneral-regs-only',
             '-mstrict-align', '-fno-common', '-fno-pic', '-fno-pie', '-fno-stack-protector']
    flags += [f'-I{path}' for path in includes]
    # The fixed loader address is outside kernel + root loaded regions. Both the
    # host validation and upstream ensure_phys_range_valid reject collisions.
    (output / 'image_start_addr.h').write_text('#define IMAGE_START_ADDR 0x44000000\n')
    headers = sorted(p for directory in (LOADER / 'config', LOADER / 'vendor') for p in directory.rglob('*.h'))
    digest = hashlib.sha256((' '.join(flags) + subprocess.check_output([cc, '--version'], text=True)).encode())
    for path in headers:
        digest.update(path.read_bytes())
    objects = []
    for index, source in enumerate(sources):
        obj = output / f'{index}.o'
        stamp = output / f'{index}.sha256'
        key = hashlib.sha256(digest.digest() + source.read_bytes()).hexdigest()
        if not obj.exists() or not stamp.exists() or stamp.read_text() != key:
            command([cc, *flags, '-c', source, '-o', obj])
            stamp.write_text(key)
        objects.append(obj)
    script = output / 'linker.lds'
    command([cc, *flags, '-P', '-E', '-x', 'c', LOADER / 'vendor/elfloader/src/linker.lds', '-o', script])
    for el2 in (True, False):
        directory = output / ('el2' if el2 else 'el1')
        directory.mkdir(exist_ok=True)
        for source, name in ((kernel, 'kernel.elf'), (root, 'rootserver')):
            command(['rust-objcopy', '--strip-all', source, directory / name])
            os.utime(directory / name, (0, 0))
        dtb = directory / 'kernel.dtb'
        dtb_stamp = directory / 'dtb.sha256'
        dtb_key = hashlib.sha256((subprocess.check_output([qemu, '--version'], text=True)
                                 + f'virt,gic-version=3,virtualization={el2},cortex-a72,smp=1,m=128M').encode()).hexdigest()
        if not dtb.exists() or not dtb_stamp.exists() or dtb_stamp.read_text() != dtb_key:
            command([qemu, '-machine', f"virt,gic-version=3,virtualization={'on' if el2 else 'off'},dumpdtb={dtb}",
                     '-cpu', 'cortex-a72', '-smp', '1', '-m', '128M', '-display', 'none', '-nic', 'none'],
                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            dtb_stamp.write_text(dtb_key)
        os.utime(dtb, (0, 0))
        validate_pair(kernel_info, root_info, int.from_bytes(dtb.read_bytes()[4:8], 'big'))
        archive = directory / 'archive.cpio'
        with archive.open('wb') as stream:
            command(['cpio', '--create', '--format=newc', '--reproducible', '--owner=0:0', '--quiet'],
                    cwd=directory, input=b'kernel.elf\nkernel.dtb\nrootserver\n', stdout=stream)
        assembly = directory / 'archive.S'
        assembly.write_text('.section ._archive_cpio,"aw"\n.globl _archive_start, _archive_start_end\n'
                            '_archive_start:\n.incbin ' + json.dumps(str(archive)) + '\n_archive_start_end:\n')
        archive_obj = directory / 'archive.o'
        command([cc, *flags, '-c', assembly, '-o', archive_obj])
        command([cc, '-nostdlib', '-static', '-no-pie', f'-Wl,-T,{script}', '-Wl,--build-id=none',
                 *objects, archive_obj, '-lgcc', '-o', directory / 'elfloader'])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('kernel', type=Path)
    parser.add_argument('root', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('--qemu', default='qemu-system-aarch64')
    parser.add_argument('--cc', default='aarch64-linux-gnu-gcc')
    args = parser.parse_args()
    build(args.kernel.resolve(), args.root.resolve(), args.output.resolve(), args.qemu, args.cc)


if __name__ == '__main__':
    main()
