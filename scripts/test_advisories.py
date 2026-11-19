import datetime
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("advisories", Path(__file__).with_name("check-advisories.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class AdvisoryTests(unittest.TestCase):
    def setUp(self):
        self.today = datetime.date(2026, 11, 16)
        self.issue = {"advisory": {"id": "RUSTSEC-2024-0429"}, "package": {"name": "glib", "version": "0.18.5"}}
        self.report = {"vulnerabilities": {"list": []}, "warnings": {"unsound": [self.issue]}}
        self.exception = {"advisory": "RUSTSEC-2024-0429", "package": "glib", "version": "0.18.5", "reason": "Reachability remains under review", "reviewBy": "2026-12-16"}

    def test_specific_exception_does_not_suppress_category_or_other_versions(self):
        self.assertEqual(module.check(self.report, [self.exception], self.today), [])
        self.issue["package"]["version"] = "0.18.6"
        self.assertEqual(len(module.check(self.report, [self.exception], self.today)), 1)
        self.issue["package"]["version"] = "0.18.5"
        self.issue["advisory"]["id"] = "RUSTSEC-2026-0001"
        self.assertEqual(len(module.check(self.report, [self.exception], self.today)), 1)

    def test_review_deadline_is_enforced(self):
        with self.assertRaises(ValueError):
            module.check(self.report, [self.exception], datetime.date(2026, 12, 16))

    def test_yanked_packages_and_vulnerabilities_fail(self):
        self.report["warnings"] = {"yanked": [{"package": {"name": "crate", "version": "1.0.0"}}]}
        self.report["vulnerabilities"]["list"] = [self.issue]
        self.assertEqual(len(module.check(self.report, [], self.today)), 2)

    def test_invalid_report_and_empty_reasons_fail(self):
        with self.assertRaises(KeyError):
            module.check({}, [], self.today)
        self.exception["reason"] = " "
        with self.assertRaises(ValueError):
            module.check(self.report, [self.exception], self.today)


if __name__ == "__main__":
    unittest.main()
