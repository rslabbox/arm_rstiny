"""Host validation of ELF inputs to the seL4-compatible boot handoff."""
import struct

PAGE = 4096
KERNEL_OFFSET = 0xffff000000000000
KERNEL_LINK_BASE = 0xffff800000000000


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


def root_layout(root, dtb_size):
    """Mirror the initial-task resource policy; ELF owns all image/stack VAs."""
    require(PAGE <= root['start'] < root['end'] <= 128 * 1024 * 1024,
            'root outside user address space')
    require(root['start'] % PAGE == root['end'] % PAGE == 0, 'unaligned root image')
    require(root['end'] - root['start'] <= 1024 * PAGE, 'root image span exceeds limit')
    require(40 <= dtb_size <= 1024 * 1024, 'invalid DTB size')
    ipc = root['end']
    boot_info = ipc + PAGE
    extra = boot_info + PAGE
    extra_size = dtb_size + 16
    end = extra + (extra_size + PAGE - 1) // PAGE * PAGE
    require(end <= 128 * 1024 * 1024, 'root metadata exceeds user address space')
    image_pages = sum((s['end'] - s['va']) // PAGE for s in root['segments'])
    require(image_pages + (end - ipc) // PAGE <= 1024, 'root mapped pages exceed limit')
    return dict(ipc=ipc, boot_info=boot_info, extra=extra, extra_size=extra_size, end=end)


def validate_pair(kernel, root, dtb_size, kernel_physical=0x40200000):
    # Input preflight, not the loader's allocation decision. PT_LOAD.p_paddr
    # does not constrain placement; kernel VA is mapped to the chosen RAM gap.
    require(KERNEL_LINK_BASE <= kernel['start'] < kernel['end'] <= KERNEL_LINK_BASE + 0x2000000,
            'kernel outside virtual image window')
    require(kernel['start'] % 0x200000 == 0, 'kernel virtual start must be block aligned')
    root_layout(root, dtb_size)
    dtb_start = kernel_physical + kernel['end'] - kernel['start']
    image_start = (dtb_start + dtb_size + PAGE - 1) // PAGE * PAGE
    image_end = image_start + root['end'] - root['start']
    require(0x40200000 <= kernel_physical and image_end + PAGE <= 0x48000000, 'loaded images exceed RAM')
    return image_start, image_end
