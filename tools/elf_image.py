"""Host validation of ELF inputs to the unmodified seL4 elfloader."""
import struct

PAGE = 4096
KERNEL_OFFSET = 0xffff000000000000


def require(condition, message):
    if not condition:
        raise ValueError(message)


def parse_elf(data):
    require(len(data) >= 64, 'truncated ELF header')
    require(data[:7] == b'\x7fELF\x02\x01\x01', 'expected little-endian ELF64')
    header = struct.unpack_from('<HHIQQQIHHHHHH', data, 16)
    kind, machine, version, entry, phoff = header[:5]
    ehsize, phsize, phnum = header[7:10]
    require((kind, machine, version, ehsize, phsize) == (2, 183, 1, 64, 56), 'unsupported ELF header')
    require(0 < phnum <= 32 and phoff + phnum * phsize <= len(data), 'invalid program headers')
    segments, pages = [], set()
    for index in range(phnum):
        typ, flags, offset, va, pa, filesz, memsz, align = struct.unpack_from('<IIQQQQQQ', data, phoff + index * phsize)
        require(typ not in (2, 3, 7), 'dynamic loading/interpreter/TLS unsupported')
        if typ != 1 or memsz == 0:
            continue
        require(flags in (4, 5, 6), 'unsupported segment permissions')
        require(filesz <= memsz and offset + filesz <= len(data), 'truncated segment')
        require(va + memsz < 2**64 and pa + memsz < 2**64, 'segment address overflow')
        require(va % PAGE == 0 and offset % PAGE == 0, 'unaligned segment')
        require(align >= PAGE and align & (align - 1) == 0 and va % align == offset % align, 'invalid alignment')
        end = (va + memsz + PAGE - 1) // PAGE * PAGE
        require(end - va <= 32 * 1024 * 1024, 'segment exceeds platform limit')
        mapped = set(range(va, end, PAGE))
        require(not pages & mapped, 'segments share pages')
        pages |= mapped
        segments.append(dict(va=va, pa=pa, end=end, flags=flags, filesz=filesz, offset=offset, memsz=memsz))
    require(entry % 4 == 0 and any(s['flags'] == 5 and s['va'] <= entry < s['va'] + s['filesz'] for s in segments), 'invalid executable entry')
    return dict(entry=entry, start=min(pages), end=max(pages) + PAGE, segments=segments)


def validate_pair(kernel, root, dtb_size):
    require(kernel['start'] == KERNEL_OFFSET + 0x40200000, 'kernel start must match boot mapping alignment')
    for segment in kernel['segments']:
        require(segment['pa'] == segment['va'] - KERNEL_OFFSET, 'inconsistent kernel physical offset')
    require(root['start'] == 0x400000 and root['end'] == 0x600000, 'root ELF must include its runtime stack')
    require(any(s['flags'] == 6 and s['va'] <= 0x5fc000 and s['end'] == 0x600000 for s in root['segments']), 'missing writable root stack')
    # The upstream loader packs the DTB after kernel memory, then root + headers.
    dtb_start = kernel['end'] - KERNEL_OFFSET
    require(40 <= dtb_size <= 1024 * 1024, 'invalid DTB size')
    image_start = (dtb_start + dtb_size + PAGE - 1) // PAGE * PAGE
    image_end = image_start + root['end'] - root['start']
    require(image_end + PAGE <= 0x42000000, 'loaded root exceeds kernel boot window')
    return image_start, image_end
