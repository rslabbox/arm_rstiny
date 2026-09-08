#!/usr/bin/env python3
"""Package kernel/DTB/rootserver CPIO and link the Rust bootloader."""
import argparse
import os
from pathlib import Path
import subprocess
import shutil
from elf_image import parse_elf, validate_pair

ROOT = Path(__file__).resolve().parents[1]
TARGET = 'aarch64-unknown-none-softfloat'


def command(args, **kwargs):
    subprocess.run([str(arg) for arg in args], check=True, **kwargs)


def build(kernel, root, output, platform, mode):
    kernel_info, root_info = parse_elf(kernel.read_bytes()), parse_elf(root.read_bytes())
    output.mkdir(parents=True, exist_ok=True)
    for source, name in ((kernel, 'kernel.elf'), (root, 'rootserver')):
        command(['rust-objcopy', '--strip-all', source, output / name])
        os.utime(output / name, (0, 0))
    dtb = output / 'kernel.dtb'
    shutil.copyfile(platform / 'kernel.dtb', dtb)
    os.utime(dtb, (0, 0))
    validate_pair(kernel_info, root_info, int.from_bytes(dtb.read_bytes()[4:8], 'big'))
    archive = output / 'archive.cpio'
    contents = subprocess.check_output(
        ['cpio', '--create', '--format=newc', '--reproducible', '--owner=0:0', '--quiet'],
        cwd=output, input=b'kernel.elf\nkernel.dtb\nrootserver\n')
    if not archive.exists() or archive.read_bytes() != contents:
        archive.write_bytes(contents)
    env = dict(os.environ, BOOT_ARCHIVE=str(archive), PLATFORM_DIR=str(platform))
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
