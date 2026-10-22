"""Check upgrade decisions against systemctl's real offline unit-file handling."""
from pathlib import Path
import os
import shutil
import subprocess
import tempfile
import unittest

SOURCE = Path(__file__).resolve().parents[1]


@unittest.skipUnless(shutil.which('systemctl') and shutil.which('bash'), 'systemd tools required')
class SleepInstallTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='ucc-sleep-units-')
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        vendor = self.root / 'usr/lib/systemd/system'
        vendor.mkdir(parents=True)
        shutil.copyfile(SOURCE / 'uccd/uccd-sleep.service', vendor / 'uccd-sleep.service')
        (vendor / 'uccd-pre-sleep.service').symlink_to('uccd-sleep.service')

    def unit_link(self, relative, target):
        link = self.root / relative
        link.parent.mkdir(parents=True, exist_ok=True)
        link.symlink_to(target)

    def enabled(self):
        # No system manager calls: all commands use the temporary --root tree.
        script = '''
source "$1"
systemctl() { command systemctl --root "$UCC_SLEEP_TEST_ROOT" "$@"; }
ucc_sleep_hook_enabled "$UCC_SLEEP_TEST_ROOT"
'''
        result = subprocess.run(['bash', '-c', script, 'test', str(SOURCE / 'ucc.install')],
                                env={**os.environ, 'UCC_SLEEP_TEST_ROOT': str(self.root)},
                                capture_output=True, text=True)
        self.assertIn(result.returncode, (0, 1), result.stderr)
        return result.returncode == 0

    def test_disabled_stays_disabled_despite_compatibility_alias(self):
        self.assertFalse(self.enabled())

    def test_existing_canonical_hook_is_migrated(self):
        self.unit_link('etc/systemd/system/suspend.target.wants/uccd-sleep.service',
                       '/usr/lib/systemd/system/uccd-sleep.service')
        self.assertTrue(self.enabled())

    def test_legacy_pre_sleep_only_is_migrated(self):
        self.unit_link('etc/systemd/system/hibernate.target.wants/uccd-pre-sleep.service',
                       '/usr/lib/systemd/system/uccd-pre-sleep.service')
        self.assertTrue(self.enabled())

    def test_mask_is_preserved_even_with_legacy_links(self):
        self.unit_link('etc/systemd/system/uccd-sleep.service', '/dev/null')
        self.unit_link('etc/systemd/system/suspend.target.wants/uccd-pre-sleep.service',
                       '/usr/lib/systemd/system/uccd-pre-sleep.service')
        self.assertFalse(self.enabled())


if __name__ == '__main__':
    unittest.main()
