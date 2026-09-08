#!/usr/bin/env python3
"""Link a userspace crate with the shared static AArch64 runtime contract."""
import argparse
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('package')
    parser.add_argument('--mode', choices=('debug', 'release'), default='debug')
    parser.add_argument('--image-base', type=lambda value: int(value, 0))
    args = parser.parse_args()
    command = ['cargo', 'rustc', '-p', args.package, '--bin', args.package,
               '--target', 'aarch64-unknown-none-softfloat',
               '--target-dir', f'target/apps/{args.mode}']
    if args.mode == 'release':
        command.append('--release')
    # Keep LLD's default ELF layout. Unlike seL4's --no-rosegment, separate
    # loadable segments retain this kernel's per-page W^X and read-only data.
    flags = ['-z', 'max-page-size=4096', '-z', 'separate-loadable-segments',
             '-z', 'norelro', '--entry=_start']
    if args.image_base is not None:
        flags.append(f'--image-base={args.image_base:#x}')
    command += ['--']
    for flag in flags:
        command += ['-C', f'link-arg={flag}']
    subprocess.run(command, check=True)


if __name__ == '__main__':
    main()
