"""Exercise the boot module parser with deliberately malformed ELF inputs."""
import struct
import unittest

from pack_root import MAGIC, pack


def elf(segments=None, entry=0x400000):
    # type, flags, offset, VA, PA, file size, memory size, alignment
    segments = segments or [(1, 5, 4096, 0x400000, 0, 4, 4096, 4096)]
    data = bytearray(8192)
    data[:7] = b'\x7fELF\x02\x01\x01'
    struct.pack_into('<HHIQQQIHHHHHH', data, 16,
                     2, 183, 1, entry, 64, 0, 0, 64, 56, len(segments), 0, 0, 0)
    for index, segment in enumerate(segments):
        struct.pack_into('<IIQQQQQQ', data, 64 + 56 * index, *segment)
    data[4096:4100] = b'code'
    return data


class PackRootTests(unittest.TestCase):
    def test_payload_and_zero_fill_description(self):
        result = pack(elf())
        self.assertEqual(struct.unpack_from('<8sQQ', result), (MAGIC, 0x400000, 1))
        self.assertEqual(struct.unpack_from('<QQQQQ', result, 24),
                         (0x400000, 4096, 4, 64, 5))
        self.assertEqual(result[64:], b'code')

    def test_rejects_truncation(self):
        for size in (0, 63, 100, 4098):
            with self.subTest(size=size), self.assertRaises(ValueError):
                pack(elf()[:size])

    def test_rejects_invalid_segments(self):
        original = (1, 5, 4096, 0x400000, 0, 4, 4096, 4096)
        for field, value in ((0, 2), (0, 3), (0, 7), (1, 7),
                             (2, 4097), (3, 0), (3, 0x500000),
                             (5, 4097), (6, 0x100001), (7, 8191)):
            segment = list(original)
            segment[field] = value
            with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                pack(elf([segment]))

    def test_rejects_shared_pages(self):
        with self.assertRaisesRegex(ValueError, 'share pages'):
            pack(elf([(1, 5, 4096, 0x400000, 0, 4, 4096, 4096),
                      (1, 6, 4096, 0x400000, 0, 4, 4096, 4096)]))

    def test_rejects_entry_in_bss_or_non_executable_memory(self):
        with self.assertRaises(ValueError):
            pack(elf(entry=0x400008))
        with self.assertRaises(ValueError):
            pack(elf([(1, 6, 4096, 0x400000, 0, 4, 4096, 4096)]))

    def test_rejects_wrong_machine(self):
        data = elf()
        struct.pack_into('<H', data, 18, 62)
        with self.assertRaises(ValueError):
            pack(data)


if __name__ == '__main__':
    unittest.main()
