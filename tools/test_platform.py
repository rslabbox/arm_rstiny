"""Verify that generated inputs agree with the overlaid QEMU DTB."""
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import build_platform as platform


class PlatformTests(unittest.TestCase):
    def test_overlay_and_cache(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            overlay = root / 'overlay.dts'
            overlay.write_text(platform.OVERLAY.read_text())
            with patch.object(platform, 'OVERLAY', overlay):
                out = root / 'output'
                platform.generate(out)
                info = json.loads((out / 'platform.json').read_text())
                self.assertEqual(info['psci_method'], 'hvc')
                self.assertEqual(info['timer_irq'], 30)
                self.assertEqual(info['kernel_devices'], ['/pl011@9000000', '/intc@8000000', '/timer'])
                raw = platform.run(['fdtget', '-t', 's', out / 'kernel.dtb', '/psci', 'method']).strip()
                self.assertEqual(raw, info['psci_method'])
                stamp = (out / 'platform.rs').stat().st_mtime_ns
                platform.generate(out)
                self.assertEqual(stamp, (out / 'platform.rs').stat().st_mtime_ns)
                overlay.write_text(overlay.read_text() + '\n/ { model = "overlay-test"; };\n')
                platform.generate(out)
                self.assertEqual(platform.run(['fdtget', '-t', 's', out / 'kernel.dtb', '/', 'model']).strip(), 'overlay-test')

    def test_rejects_missing_kernel_gic(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            overlay = root / 'overlay.dts'
            overlay.write_text(platform.OVERLAY.read_text().replace('&{/intc@8000000},', ''))
            with patch.object(platform, 'OVERLAY', overlay):
                with self.assertRaisesRegex(ValueError, 'arm,gic-v3'):
                    platform.generate(root / 'output')


if __name__ == '__main__':
    unittest.main()
