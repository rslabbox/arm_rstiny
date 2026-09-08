#!/usr/bin/env python3
"""Package kernel/DTB/rootserver CPIO and link the Rust bootloader."""
import argparse
import os
from pathlib import Path
import subprocess
import shutil
import tempfile
from elf_image import parse_elf, validate_pair

ROOT = Path(__file__).resolve().parents[1]
TARGET = 'aarch64-unknown-none-softfloat'


def command(args, **kwargs):
    subprocess.run([str(arg) for arg in args], check=True, **kwargs)


def archive_object(archive):
    """Convert archive.cpio into a deterministic, read-only AArch64 object.

    The fixed input basename keeps objcopy's generated symbols independent of
    the build directory. Replace the object only when its bytes change so Cargo
    can use the object itself as the incremental link dependency.
    """
    archive = archive.resolve()
    if archive.name != 'archive.cpio':
        raise ValueError('archive input must be named archive.cpio')
    output = archive.with_name('archive.o')
    with tempfile.TemporaryDirectory(prefix='archive-object-', dir=archive.parent) as temporary:
        candidate = Path(temporary) / 'archive.o'
        command([
            'rust-objcopy', '--input-target=binary', '--output-target=elf64-littleaarch64',
            '--binary-architecture=aarch64',
            '--rename-section=.data=.boot_archive,alloc,load,readonly,data,contents',
            '--set-section-alignment=.data=4',
            archive.name, candidate,
        ], cwd=archive.parent)
        if not output.exists() or output.read_bytes() != candidate.read_bytes():
            candidate.replace(output)
    return output


def build(kernel, root, output, platform, mode):
    kernel_info, root_info = parse_elf(kernel.read_bytes()), parse_elf(root.read_bytes())
    output.mkdir(parents=True, exist_ok=True)
    for source, name in ((kernel, 'kernel.elf'), (root, 'rootserver')):
        command(['rust-objcopy', '--strip-all', source, output / name])
        os.chmod(output / name, 0o644)
        os.utime(output / name, (0, 0))
    dtb = output / 'kernel.dtb'
    shutil.copyfile(platform / 'kernel.dtb', dtb)
    os.chmod(dtb, 0o644)
    os.utime(dtb, (0, 0))
    validate_pair(kernel_info, root_info, int.from_bytes(dtb.read_bytes()[4:8], 'big'))
    archive = output / 'archive.cpio'
    contents = subprocess.check_output(
        ['cpio', '--create', '--format=newc', '--reproducible', '--owner=0:0', '--quiet'],
        cwd=output, input=b'kernel.elf\nkernel.dtb\nrootserver\n')
    if not archive.exists() or archive.read_bytes() != contents:
        archive.write_bytes(contents)
    obj = archive_object(archive)
    env = dict(os.environ, BOOT_ARCHIVE_OBJECT=str(obj))
    build_dir = output / 'build'
    flags = ['cargo', 'build', '-p', 'bootloader', '--target', TARGET, '--target-dir', build_dir]
    if mode == 'release':
        flags.append('--release')
    command(flags, cwd=ROOT, env=env)
    shutil.copyfile(build_dir / TARGET / mode / 'bootloader', output / 'bootloader')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('kernel', type=Path)
    parser.add_argument('root', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('--platform', type=Path, required=True)
    parser.add_argument('--mode', choices=['debug', 'release'], default='debug')
    args = parser.parse_args()
    build(args.kernel.resolve(), args.root.resolve(), args.output.resolve(),
          args.platform.resolve(), args.mode)


if __name__ == '__main__':
    main()
