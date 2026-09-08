"""Verify archive objects and Cargo's real incremental link dependency."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

from build_image import ROOT, TARGET, archive_object


def command(args, **kwargs):
    return subprocess.run([str(arg) for arg in args], check=True,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE, **kwargs)


def payload(image, directory):
    output = directory / 'payload.bin'
    command(['rust-objcopy', f'--dump-section=.boot_archive={output}', image])
    return output.read_bytes()


class BootArchiveTests(unittest.TestCase):
    def test_object_is_reproducible_and_read_only(self):
        with tempfile.TemporaryDirectory(prefix='rstiny-archive-') as tmp:
            directory = Path(tmp)
            archive = directory / 'archive.cpio'
            archive.write_bytes(b'archive payload')
            obj = archive_object(archive)
            original, stamp = obj.read_bytes(), obj.stat().st_mtime_ns
            archive_object(archive)
            self.assertEqual(obj.stat().st_mtime_ns, stamp)
            self.assertEqual(payload(obj, directory), archive.read_bytes())
            metadata = command(['rust-readobj', '--file-headers', '--sections', obj]).stdout.decode()
            self.assertIn('Type: Relocatable', metadata)
            self.assertIn('Machine: EM_AARCH64', metadata)
            section = metadata.split('Name: .boot_archive', 1)[1]
            self.assertIn('SHF_ALLOC', section)
            self.assertNotIn('SHF_WRITE', section)
            self.assertNotIn('SHF_EXECINSTR', section)
            self.assertIn('AddressAlignment: 4', section)
            other = directory / 'other directory'
            other.mkdir()
            copy = other / 'archive.cpio'
            copy.write_bytes(archive.read_bytes())
            self.assertEqual(archive_object(copy).read_bytes(), original)

    def test_relinks_changed_object_and_rejects_missing_archive(self):
        with tempfile.TemporaryDirectory(prefix='rstiny-link-') as tmp:
            directory = Path(tmp)
            archive = directory / 'archive.cpio'
            # Link-only fixtures: runtime CPIO validity is checked in QEMU tests.
            archive.write_bytes(b'first archive')
            obj = archive_object(archive)
            # Standalone bootloader linking must not invoke platform generation.
            env = dict(os.environ, BOOT_ARCHIVE_OBJECT=str(obj),
                       PLATFORM_DIR=str(directory / 'absent-platform'),
                       QEMU=str(directory / 'absent-qemu'))
            target = directory / 'target'
            args = ['cargo', 'build', '-p', 'bootloader', '--target', TARGET, '--target-dir', target]
            executable = target / TARGET / 'debug' / 'bootloader'
            command(args, cwd=ROOT, env=env)
            self.assertEqual(payload(executable, directory), archive.read_bytes())
            archive.write_bytes(b'other archive')  # Same path and size, different bytes.
            archive_object(archive)
            command(args, cwd=ROOT, env=env)
            self.assertEqual(payload(executable, directory), archive.read_bytes())
            del env['BOOT_ARCHIVE_OBJECT']
            result = subprocess.run(args, cwd=ROOT, env=env, capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn(b'missing boot archive', result.stderr)
