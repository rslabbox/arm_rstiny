#!/usr/bin/env python3
"""Build the unmodified seL4 ARM elfloader and its kernel/DTB/rootserver CPIO."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import shutil
from elf_image import parse_elf, validate_pair

ROOT = Path(__file__).resolve().parents[1]
LOADER = ROOT / 'loader'


def command(args, **kwargs):
    subprocess.run([str(arg) for arg in args], check=True, **kwargs)


def build(kernel, root, output, platform, cc):
    manifest = json.loads((LOADER / 'upstream.json').read_text())
    for name, checksum in manifest['sha256'].items():
        if hashlib.sha256((LOADER / 'vendor' / name).read_bytes()).hexdigest() != checksum:
            raise ValueError(f'upstream loader source changed: {name}')
    kernel_info, root_info = parse_elf(kernel.read_bytes()), parse_elf(root.read_bytes())
    output.mkdir(parents=True, exist_ok=True)
    sources = [LOADER / name for name in json.loads((LOADER / 'sources.json').read_text())]
    includes = [platform, LOADER / 'config', output, LOADER / 'vendor/libcpio/include']
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
    headers = sorted(p for directory in (platform, LOADER / 'config', LOADER / 'vendor') for p in directory.rglob('*.h'))
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
    directory = output
    for source, name in ((kernel, 'kernel.elf'), (root, 'rootserver')):
        command(['rust-objcopy', '--strip-all', source, directory / name])
        os.utime(directory / name, (0, 0))
    dtb = directory / 'kernel.dtb'
    shutil.copyfile(platform / 'kernel.dtb', dtb)
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
    parser.add_argument('--platform', type=Path, required=True)
    parser.add_argument('--cc', default='aarch64-linux-gnu-gcc')
    args = parser.parse_args()
    build(args.kernel.resolve(), args.root.resolve(), args.output.resolve(),
          args.platform.resolve(), args.cc)


if __name__ == '__main__':
    main()
