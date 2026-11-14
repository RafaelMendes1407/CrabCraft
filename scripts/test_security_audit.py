"""Guard the severity policy against accidental promotion of unknown data."""

import unittest
from unittest.mock import patch

from scripts.security_audit import advisory_severity, static


class SeverityTests(unittest.TestCase):
    def test_cvss_boundaries(self):
        cases = ((0, "INFORMATIONAL"), (0.1, "LOW"), (3.9, "LOW"),
                 (4, "MEDIUM"), (6.9, "MEDIUM"), (7, "HIGH"),
                 (8.9, "HIGH"), (9, "CRITICAL"), (10, "CRITICAL"))
        for score, expected in cases:
            with self.subTest(score=score):
                self.assertEqual(advisory_severity({"cvss": score}), expected)

    def test_untrusted_cvss_formats_remain_unspecified(self):
        for score in (None, "9.8", "CVSS:3.1/AV:N", True, -1, 10.1, float("nan")):
            with self.subTest(score=score):
                self.assertEqual(advisory_severity({"cvss": score}), "UNSPECIFIED")

    def test_static_failures_are_never_downgraded(self):
        for fmt, clippy in ((1, 0), (0, 101), (1, 101)):
            checks = [{"name": name, "status": "FAIL" if code else "PASS", "exit_code": code}
                      for name, code in (("cargo-fmt", fmt), ("cargo-clippy", clippy))]
            with self.subTest(fmt=fmt, clippy=clippy), patch(
                    "scripts.security_audit.command_result", side_effect=checks):
                self.assertEqual(static()["status"], "FAIL")


if __name__ == "__main__":
    unittest.main()
