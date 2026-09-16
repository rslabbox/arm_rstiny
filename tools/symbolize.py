#!/usr/bin/env python3
"""Symbolize raw addresses against one or more ELF files (kernel or apps).

    python3 tools/symbolize.py --elf target/kernel/.../kernel \\
        --elf target/apps/release/mysh.elf ffff800000027090 0x201388

Addresses may be pasted in any form (0x..., bare hex, with the leading
ffff... kernel prefix). Symbols come from the ELF symbol tables; the
ELF whose [lowest, highest) text range contains the address wins. Pairs
well with the backtraces and task dumps the kernel prints on faults.
"""
import argparse
import subprocess
from pathlib import Path


def load_symbols(elf):
    out = subprocess.run(['rust-nm', '--defined-only', str(elf)],
                         capture_output=True, text=True)
    symbols = []
    for line in out.stdout.splitlines():
        fields = line.split(maxsplit=2)
        if len(fields) == 3 and all(c in '0123456789abcdefABCDEF' for c in fields[0]):
            symbols.append((int(fields[0], 16), fields[2]))
    symbols.sort()
    return symbols


def demangle(name):
    # Legacy Rust mangling: _R...; trim hash markers for readability.
    if name.startswith('_RN'):
        return name
    return name


def resolve(symbols, address):
    best = None
    for addr, name in symbols:
        if addr <= address:
            best = (addr, name)
        else:
            break
    if best is None:
        return f'{address:#x} = ?'
    base, name = best
    return f'{address:#x} = {name}+{address - base:#x}'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--elf', action='append', default=[],
                        help='ELF to load symbols from (repeatable)')
    parser.add_argument('addresses', nargs='*',
                        help='hex addresses (0x prefix optional)')
    args = parser.parse_args()

    tables = []
    for elf in args.elf:
        path = Path(elf)
        if not path.exists():
            raise SystemExit(f'missing ELF: {elf}')
        tables.append((path, load_symbols(path)))

    addresses = args.addresses
    if not addresses:
        addresses = [line.strip() for line in sys.stdin if line.strip()]

    for text in addresses:
        text = text.strip().replace(',', '')
        if not text:
            continue
        value = int(text, 16) if not text.isdigit() else int(text)
        for path, symbols in tables:
            if symbols and symbols[0][0] <= value <= symbols[-1][0]:
                print(resolve(symbols, value))
                break
        else:
            print(f'{value:#x} = ? (no ELF covers this range)')


if __name__ == '__main__':
    main()
