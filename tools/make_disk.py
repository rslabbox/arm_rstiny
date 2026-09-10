#!/usr/bin/env python3
"""Build a bare FAT32 disk image with mtools (no partition table): sector 0 is
the BPB the fs-server mounts (docs/disk-driver.md section 5.3).

    python3 tools/make_disk.py disk.img --file HELLO.ELF=path/to/hello.elf

Names must be valid 8.3 short names. Corruption switches build images for the
fs-server failure-path acceptance:
  --corrupt-bpb  invalidates the boot sector signature
  --cycle-fat    makes the first cluster of HELLO.ELF point at itself
  --truncate     cuts the image short so late reads fall off the end
"""
import argparse
import shutil
import struct
import subprocess
import tempfile
from pathlib import Path


def check_name(name):
    stem, _, extension = name.partition('.')
    if not stem.isalnum() or not stem.isupper() or len(stem) > 8 or len(extension) > 3:
        raise SystemExit(f'not an 8.3 short name: {name}')


def bpb_fields(image):
    boot = bytearray(image.read(512))
    if struct.unpack_from('<H', boot, 0x1FE)[0] != 0xAA55:
        raise SystemExit('image has no boot sector signature')
    return {
        'bytes_per_sector': struct.unpack_from('<H', boot, 0x0B)[0],
        'reserved_sectors': struct.unpack_from('<H', boot, 0x0E)[0],
        'sectors_per_cluster': boot[0x0D],
        'num_fats': boot[0x10],
        'fat_size': struct.unpack_from('<I', boot, 0x24)[0],
        'root_cluster': struct.unpack_from('<I', boot, 0x2C)[0],
    }


def directory_entry(image, fields, short_name):
    """Return the 32-byte root directory entry for `short_name` (8.3)."""
    data_start = fields['reserved_sectors'] + fields['num_fats'] * fields['fat_size']
    root_start = data_start + (fields['root_cluster'] - 2) * fields['sectors_per_cluster']
    spc = fields['sectors_per_cluster']
    stem, _, extension = short_name.partition('.')
    raw = f'{stem:<8}{extension:<3}'.encode('ascii')
    for sector in range(root_start, root_start + spc):
        image.seek(sector * fields['bytes_per_sector'])
        block = image.read(fields['bytes_per_sector'])
        for offset in range(0, len(block), 32):
            if block[offset:offset + 11] == raw:
                return block[offset:offset + 32]
    raise SystemExit(f'{short_name} not found in the root directory')


def cycle_fat(image, fields, short_name='HELLO.ELF'):
    """Point the file's first cluster at itself: a FAT loop a reader must bound."""
    entry = directory_entry(image, fields, short_name)
    first = struct.unpack_from('<H', entry, 0x1A)[0] | \
        struct.unpack_from('<H', entry, 0x14)[0] << 16
    fat_offset = fields['reserved_sectors'] * fields['bytes_per_sector'] + first * 4
    image.seek(fat_offset)
    image.write(struct.pack('<I', first))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--size-mb', type=int, default=64)
    parser.add_argument('--file', action='append', default=[],
                        help='NAME=PATH short-name pairs copied into the root directory')
    parser.add_argument('--corrupt-bpb', action='store_true')
    parser.add_argument('--cycle-fat', action='store_true')
    parser.add_argument('--truncate', action='store_true')
    args = parser.parse_args()
    for tool in ('mkfs.fat', 'mcopy'):
        if shutil.which(tool) is None:
            raise SystemExit(f'{tool} is required (install dosfstools and mtools)')
    files = []
    for entry in args.file:
        name, _, path = entry.partition('=')
        check_name(name)
        files.append((name, Path(path)))

    args.output.parent.mkdir(parents=True, exist_ok=True)
    # 64 MiB at 512 B sectors with one sector per cluster stays above the
    # 65525-cluster minimum that mkfs.fat enforces for FAT32. The file is
    # pre-sized so mkfs.fat discovers the geometry from it.
    with tempfile.NamedTemporaryFile(suffix='.img', delete=False) as handle:
        handle.truncate(args.size_mb * 1024 * 1024)
        temporary = Path(handle.name)
    try:
        subprocess.run(['mkfs.fat', '-F', '32', '-S', '512', '-s', '1',
                        '-n', 'RSTINY', str(temporary)],
                       check=True, stdout=subprocess.DEVNULL)
        for name, path in files:
            subprocess.run(['mcopy', '-i', str(temporary), str(path), f'::{name}'],
                           check=True, stdout=subprocess.DEVNULL)

        if args.corrupt_bpb:
            with temporary.open('r+b') as image:
                image.seek(0x1FE)
                image.write(b'\x00\x00')
        if args.cycle_fat:
            with temporary.open('r+b') as image:
                cycle_fat(image, bpb_fields(image))
        if args.truncate:
            with temporary.open('r+b') as image:
                image.truncate(args.size_mb * 1024 * 1024 // 2)
        args.output.write_bytes(temporary.read_bytes())
    finally:
        temporary.unlink(missing_ok=True)
    print(f'disk image: {args.output} ({args.output.stat().st_size} bytes)')


if __name__ == '__main__':
    main()
