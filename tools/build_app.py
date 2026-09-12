#!/usr/bin/env python3
"""Link a userspace binary shared AArch64 runtime contract.

Rust packages go through `cargo rustc` with the static runtime flags
(16-byte-aligned LOAD segments, `_start` entry, per-page W^X).

`--lang c` builds a C application from `projects/apps/<package>/` instead: it compiles
`src/*.c` and `src/*.S` with the cross gcc, links the `rstiny-alloc` staticlib
(this repo's C-ABI `malloc`/`free`, interpreter-app.md 决策 B) plus the
package's own `link.ld`, and emits the same ELF path Rust apps use so the
Makefile's strip/disk integration is identical.

`--lang python` builds the MicroPython port (ports/micropython-rstiny): it
ensures the rstiny-alloc staticlib, then runs that port's own Makefile (py
core + qstr generation, docs/micropython-port.md §3) and emits
`target/apps/<mode>/python.elf` the same way.
"""
import argparse
import glob
import os
import shutil
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CC = os.environ.get('CC_CROSS', 'aarch64-linux-gnu-gcc')
TARGET = 'aarch64-unknown-none-softfloat'


def run(command):
    subprocess.run(command, check=True)


def build_rust(package, mode, image_base):
    command = ['cargo', 'rustc', '-p', package, '--bin', package,
               '--target', TARGET, '--target-dir', f'target/apps/{mode}']
    if mode == 'release':
        command.append('--release')
    # Keep LLD's default ELF layout. Unlike seL4's --no-rosegment, separate
    # loadable segments retain this kernel's per-page W^X and read-only data.
    flags = ['-z', 'max-page-size=4096', '-z', 'separate-loadable-segments',
             '-z', 'norelro', '--entry=_start']
    if image_base is not None:
        flags.append(f'--image-base={image_base:#x}')
    command += ['--']
    for flag in flags:
        command += ['-C', f'link-arg={flag}']
    run(command)


def ensure_alloc(mode):
    """Build the rstiny-alloc C-ABI static library (interpreter-app.md 决策 B).
    Exports malloc/calloc/realloc/free/malloc_usable_size and a panic handler
    for freestanding C binaries (Rust tasks bring their own, so the handler is
    gated behind the `c-heap-panic` feature).
    """
    out_dir = ROOT / 'target' / 'apps' / mode / TARGET / mode
    out_dir.mkdir(parents=True, exist_ok=True)
    command = ['cargo', 'rustc', '-p', 'rstiny-alloc', '--crate-type', 'staticlib',
               '--features', 'c-heap-panic',
               '--target', TARGET, '--target-dir', f'target/apps/{mode}']
    if mode == 'release':
        command.append('--release')
    run(command)
    return out_dir / 'librstiny_alloc.a'


def strip_elf(path):
    """Strip a stripped ELF in place, mirroring the Rust-app pipeline."""
    strip = os.environ.get('CROSS_STRIP', 'aarch64-linux-gnu-strip')
    if shutil.which(strip):
        run([strip, '--strip-all', str(path)])


def build_c(package, mode):
    """Cross-compile `projects/apps/<package>` and link against rstiny-alloc."""
    src = ROOT / 'projects/apps' / package
    if not src.is_dir():
        raise SystemExit(f'no C sources at projects/apps/{package}')
    out_dir = ROOT / 'target' / 'apps' / mode / TARGET / mode
    out_dir.mkdir(parents=True, exist_ok=True)
    work = out_dir / f'c-{package}'
    work.mkdir(parents=True, exist_ok=True)

    # 1. rstiny-alloc as a C-ABI static library.
    alloc_archive = ensure_alloc(mode)

    # 2. Compile the C/asm sources.
    cflags = ['-nostdlib', '-nostartfiles', '-ffreestanding',
              '-fno-stack-protector', '-fno-unwind-tables', '-fno-pic',
              '-fno-asynchronous-unwind-tables', '-mno-outline-atomics',
              # No FPU context at EL0 (interpreter-app.md): forbid FP/SIMD
              # instructions entirely, or -O2 vectorises memset-family loops
              # into NEON `movi` traps.
              '-mgeneral-regs-only',
              '-Wall', '-Wextra']
    if mode == 'release':
        cflags += ['-O2']
    else:
        cflags += ['-O0', '-g']
    sources = sorted(glob.glob(str(src / 'src' / '*.c'))
                     + glob.glob(str(src / 'src' / '*.S')))
    if not sources:
        raise SystemExit(f'no src/*.c or src/*.S in projects/apps/{package}')
    objects = []
    for source in sources:
        obj = work / (Path(source).name + '.o')
        run([CC, *cflags, '-c', source, '-o', str(obj)])
        objects.append(str(obj))

    # 3. Link with the package's link.ld (ENTRY(_start), base 0x200000).
    script = src / 'link.ld'
    if not script.is_file():
        raise SystemExit(f'missing projects/apps/{package}/link.ld')
    ldflags = cflags + ['-T', str(script)]
    out_elf = out_dir / package
    run([CC, *ldflags, *objects, str(alloc_archive), '-o', str(out_elf)])

    # 4. Strip like the Rust apps (Makefile does this too, but keep the
    #    intermediate honest for `make run` paths that skip it).
    strip_elf(out_elf)


def build_python(package, mode):
    """Build the MicroPython port and emit `target/apps/<mode>/python.elf`.
    (docs/micropython-port.md §3/§10): the port Makefile compiles the py core
    and links rstiny-alloc; we copy the ELF into the shared app target dir.
    """
    out_dir = ROOT / 'target' / 'apps' / mode / TARGET / mode
    out_dir.mkdir(parents=True, exist_ok=True)
    ensure_alloc(mode)
    port = ROOT / 'ports/micropython-rstiny'
    if not (port / 'Makefile').is_file():
        raise SystemExit(f'no MicroPython port at {port}')
    run(['make', '-C', str(port), f'MODE={mode}', '-j8'])
    src_elf = port / 'build/python.elf'
    if not src_elf.is_file():
        raise SystemExit(f'missing build output {src_elf}')
    out_elf = out_dir / package
    shutil.copyfile(src_elf, out_elf)
    strip_elf(out_elf)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('package')
    parser.add_argument('--mode', choices=('debug', 'release'), default='debug')
    parser.add_argument('--image-base', type=lambda value: int(value, 0))
    parser.add_argument('--lang', choices=('rust', 'c', 'python'), default='rust')
    args = parser.parse_args()
    if args.lang == 'c':
        build_c(args.package, args.mode)
    elif args.lang == 'python':
        build_python(args.package, args.mode)
    else:
        build_rust(args.package, args.mode, args.image_base)


if __name__ == '__main__':
    main()
