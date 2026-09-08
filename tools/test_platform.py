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


if __name__ == '__main__':
    unittest.main()
