"""Verify generated inputs against the native QEMU device tree."""
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import build_platform as platform


class PlatformTests(unittest.TestCase):
    def test_native_tree_and_cache(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            platform.generate(out)
            info = json.loads((out / 'platform.json').read_text())
            self.assertEqual(info['psci_method'], 'hvc')
            self.assertEqual(info['timer_irq'], 30)
            self.assertEqual(info['kernel_devices'], ['/pl011@9000000', '/intc@8000000', '/timer'])
            self.assertEqual(info['virtio_slots'], 32)
            self.assertEqual(info['VIRTIO_MMIO_BASE'], 0x0a000000)
            self.assertEqual(info['VIRTIO_MMIO_SIZE'], 0x4000)
            # Per-line table (docs/irq.md §3.1): 32 edge VirtIO slot lines
            # (INTID 48..79) first, then the level PL011 line (INTID 33).
            irq_lines = info['irq_lines']
            self.assertEqual(len(irq_lines), 33)
            self.assertEqual(irq_lines[0], [48, 0, 0])
            self.assertEqual(irq_lines[31], [79, 0, 0])
            self.assertEqual(irq_lines[-1], [33, 1, 1])
            self.assertIn('pub const IRQ_LINES', (out / 'platform.rs').read_text())
            method = platform.run(['fdtget', '-t', 's', out / 'kernel.dtb', '/psci', 'method']).strip()
            self.assertEqual(method, info['psci_method'])
            self.assertNotIn('seL4,kernel-devices', (out / 'kernel.dts').read_text())
            stamp = (out / 'platform.rs').stat().st_mtime_ns
            platform.generate(out)
            self.assertEqual(stamp, (out / 'platform.rs').stat().st_mtime_ns)
            (out / 'kernel.dtb').unlink()
            platform.generate(out)
            self.assertTrue((out / 'kernel.dtb').exists())
            self.assertEqual(info, json.loads((out / 'platform.json').read_text()))

    def test_rejects_missing_gic(self):
        original = platform.run
        def missing_gic(args):
            result = original(args)
            if args[0] == 'fdtget' and args[-1] == 'compatible':
                result = result.replace('arm,gic-v3', 'unsupported-gic')
            return result
        with tempfile.TemporaryDirectory() as tmp, patch.object(platform, 'run', missing_gic):
            with self.assertRaisesRegex(ValueError, 'arm,gic-v3'):
                platform.generate(Path(tmp))

    def test_rejects_bad_virtio_window(self):
        original = platform.run
        def short_window(args):
            result = original(args)
            # Rewrite the last virtio slot's reg so the window is non-contiguous.
            if args[0] == 'fdtget' and args[-1] == 'reg' and '/virtio_mmio@a003e00' in args:
                result = '0x0 0x0b000000 0x0 0x200'
            return result
        with tempfile.TemporaryDirectory() as tmp, patch.object(platform, 'run', short_window):
            with self.assertRaisesRegex(ValueError, 'virtio-mmio'):
                platform.generate(Path(tmp))


if __name__ == '__main__':
    unittest.main()
