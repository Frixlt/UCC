"""An explicit GUI launch exception must never hide unrelated failing tests."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

SOURCE = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('prepare_deployment', SOURCE / 'contrib/frixit/prepare-deployment.py')
prepare = importlib.util.module_from_spec(spec)
spec.loader.exec_module(prepare)


class ReportsTest(unittest.TestCase):
    def test_gui_failure_requires_explicit_exception(self):
        with tempfile.TemporaryDirectory() as directory:
            report = Path(directory) / 'tests.xml'
            report.write_text('<testsuite><testcase name="test_gui_audit"><failure/></testcase></testsuite>')
            with self.assertRaises(ValueError):
                prepare.test_report(report)
            self.assertEqual(prepare.test_report(report, allow_gui_failure=True), 1)

    def test_other_failures_and_empty_reports_cannot_be_bypassed(self):
        with tempfile.TemporaryDirectory() as directory:
            report = Path(directory) / 'tests.xml'
            for xml in ('<testsuite/>',
                        '<testsuite><testcase name="test_power_policy"><failure/></testcase></testsuite>',
                        '<testsuite><testcase name="test_gui_audit"><failure/></testcase>'
                        '<testcase name="test_import_check"><error/></testcase></testsuite>'):
                report.write_text(xml)
                with self.assertRaises(ValueError):
                    prepare.test_report(report, allow_gui_failure=True)


if __name__ == '__main__':
    unittest.main()
