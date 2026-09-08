#!/usr/bin/env python3
"""Validate a static AArch64 root ELF and pack its load segments for the kernel."""
import argparse
from pathlib import Path
import struct

IMAGE_START, IMAGE_END = 0x400000, 0x500000
PAGE = 4096
MAGIC = b'RSTROOT\0'


def pack(elf):
    def require(condition, message):
        if not condition:
            raise ValueError(message)
    require(len(elf) >= 64, 'truncated ELF header')
    require(elf[:7] == b'\x7fELF\x02\x01\x01', 'expected little-endian ELF64')
    fields = struct.unpack_from('<HHIQQQIHHHHHH', elf, 16)
    kind, machine, version, entry, phoff = fields[:5]
    ehsize, phsize, phnum = fields[7:10]
    require((kind, machine, version, ehsize, phsize) == (2, 183, 1, 64, 56), 'unsupported ELF type/header')
    require(0 < phnum <= 32 and phoff + phnum * phsize <= len(elf), 'invalid program headers')
    segments, occupied = [], set()
    for index in range(phnum):
        typ, flags, offset, va, _, filesz, memsz, align = struct.unpack_from('<IIQQQQQQ', elf, phoff + index * phsize)
        require(typ not in (2, 3, 7), 'dynamic loading, interpreter and TLS are unsupported')
        if typ != 1 or memsz == 0:
            continue
        require(flags in (4, 5, 6), 'load segment must be R, RX or RW')
        require(filesz <= memsz and offset + filesz <= len(elf), 'truncated load segment')
        require(IMAGE_START <= va < va + memsz <= IMAGE_END, 'load segment outside user image window')
        require(va % PAGE == 0 and offset % PAGE == 0, 'load segments must start on page boundaries')
        require(align >= PAGE and align & (align - 1) == 0 and va % align == offset % align, 'invalid segment alignment')
        pages = set(range(va, (va + memsz + PAGE - 1) // PAGE * PAGE, PAGE))
        require(not pages & occupied, 'load segments share pages')
        occupied |= pages
        segments.append((va, memsz, flags, elf[offset:offset + filesz]))
    require(any(flags == 5 and va <= entry < va + len(data) for va, _, flags, data in segments), 'entry outside initialized executable segment')
    header = struct.pack('<8sQQ', MAGIC, entry, len(segments))
    cursor = len(header) + 40 * len(segments)
    records, payload = bytearray(), bytearray()
    for va, memsz, flags, data in segments:
        records += struct.pack('<QQQQQ', va, memsz, len(data), cursor, flags)
        payload += data
        cursor += len(data)
    return header + records + payload


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('elf', type=Path)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    data = pack(args.elf.read_bytes())
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_bytes(data)


if __name__ == '__main__':
    main()
