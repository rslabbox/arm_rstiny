"""Exercise the ELF input validator with deliberately malformed ELF inputs."""
import struct
import unittest

from elf_image import KERNEL_LINK_BASE, parse_elf as pack, validate_pair, root_layout


def elf(segments=None, entry=0x10000):
    # type, flags, offset, VA, PA, file size, memory size, alignment
    segments = segments or [(1, 5, 4096, 0x10000, 0, 4, 4096, 4096)]
    data = bytearray(8192)
    data[:7] = b'\x7fELF\x02\x01\x01'
    struct.pack_into('<HHIQQQIHHHHHH', data, 16,
                     2, 183, 1, entry, 64, 0, 0, 64, 56, len(segments), 0, 0, 0)
    for index, segment in enumerate(segments):
        struct.pack_into('<IIQQQQQQ', data, 64 + 56 * index, *segment)
    data[4096:4100] = b'code'
    return data


class ElfImageTests(unittest.TestCase):
    def test_loader_placement(self):
        kernel = pack(elf([(1, 5, 4096, KERNEL_LINK_BASE,
                            0x40200000, 4, 4096, 4096)],
                          entry=KERNEL_LINK_BASE))
        root = pack(elf([(1, 5, 4096, 0x10000, 0, 4, 4096, 4096),
                         (1, 6, 8192, 0x13000, 0, 0, 0x4000, 4096)]))
        self.assertEqual(validate_pair(kernel, root, 41), (0x40202000, 0x40209000))
        for size in (0, 39, 1024 * 1024 + 1):
            with self.subTest(size=size), self.assertRaises(ValueError):
                validate_pair(kernel, root, size)
        # The stack belongs to the executable: no prescribed VA or PT_LOAD.
        self.assertEqual(validate_pair(kernel, pack(elf()), 40), (0x40202000, 0x40203000))
        kernel['end'] = KERNEL_LINK_BASE + 0x2001000
        with self.assertRaisesRegex(ValueError, 'virtual image window'):
            validate_pair(kernel, root, 40)
        kernel['end'] = KERNEL_LINK_BASE + 4096
        kernel['segments'][0]['pa'] += 4096
        self.assertEqual(validate_pair(kernel, root, 40), (0x40202000, 0x40209000))

    def test_root_metadata_bounds(self):
        for base in (0x10000, 0x200000, 0x3000000):
            root = pack(elf([(1, 5, 4096, base, 0, 4, 4096, 4096)], entry=base))
            layout = root_layout(root, 4096)
            self.assertEqual((layout['ipc'], layout['boot_info'], layout['extra'], layout['end']),
                             (base + 4096, base + 8192, base + 12288, base + 20480))
        for base, size in ((0, 4096), (0x7fff000, 4096), (0x10000, 1024 * 4096)):
            with self.subTest(base=base, size=size), self.assertRaises(ValueError):
                root_layout(pack(elf([(1, 5, 4096, base, 0, 4, size, 4096)], entry=base)), 40)

    def test_valid_load_metadata(self):
        result = pack(elf())
        self.assertEqual((result['entry'], result['start'], result['end']), (0x10000, 0x10000, 0x11000))
        self.assertEqual(result['segments'][0]['filesz'], 4)

    def test_rejects_truncation(self):
        for size in (0, 63, 100, 4098):
            with self.subTest(size=size), self.assertRaises(ValueError):
                pack(elf()[:size])

    def test_rejects_invalid_segments(self):
        original = (1, 5, 4096, 0x10000, 0, 4, 4096, 4096)
        for field, value in ((0, 2), (0, 3), (0, 7), (1, 7),
                             (2, 4097), (3, 2**64 - 4096),
                             (5, 4097), (6, 0x2000001), (7, 8191)):
            segment = list(original)
            segment[field] = value
            with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                pack(elf([segment]))

    def test_rejects_shared_pages(self):
        with self.assertRaisesRegex(ValueError, 'share pages'):
            pack(elf([(1, 5, 4096, 0x10000, 0, 4, 4096, 4096),
                      (1, 6, 4096, 0x10000, 0, 4, 4096, 4096)]))

    def test_rejects_entry_in_bss_or_non_executable_memory(self):
        with self.assertRaises(ValueError):
            pack(elf(entry=0x10008))
        with self.assertRaises(ValueError):
            pack(elf([(1, 6, 4096, 0x10000, 0, 4, 4096, 4096)]))

    def test_rejects_wrong_machine(self):
        data = elf()
        struct.pack_into('<H', data, 18, 62)
        with self.assertRaises(ValueError):
            pack(data)


if __name__ == '__main__':
    unittest.main()
